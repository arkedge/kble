use anyhow::Result;
use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use notalawyer_clap::*;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

use std::path::PathBuf;

#[derive(Subcommand, Debug)]
enum Commands {
    Record { output_dir: PathBuf },
    Replay { input_file: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr),
        )
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse_with_license_notice(include_notice!());
    match args.command {
        Commands::Record { output_dir } => run_record(&output_dir).await,
        Commands::Replay { input_file } => run_replay(&input_file).await,
    }
}

use std::borrow::Cow;
struct DumpRecord<'a> {
    timestamp: std::time::SystemTime,
    data: Cow<'a, [u8]>,
}

impl<'a> DumpRecord<'a> {
    fn now(data: &'a [u8]) -> Self {
        Self {
            timestamp: std::time::SystemTime::now(),
            data: Cow::Borrowed(data),
        }
    }

    fn write_to(&self, mut writer: impl std::io::Write) -> anyhow::Result<()> {
        let since_epoch = self.timestamp.duration_since(std::time::UNIX_EPOCH)?;
        writer.write_all(&since_epoch.as_secs().to_le_bytes())?;
        writer.write_all(&since_epoch.subsec_nanos().to_le_bytes())?;
        writer.write_all(&self.data[..])?;
        Ok(())
    }

    fn read_from(mut reader: impl std::io::Read) -> anyhow::Result<Self> {
        let mut secs = [0u8; 8];
        reader.read_exact(&mut secs)?;
        let secs = u64::from_le_bytes(secs);
        let mut nanos = [0u8; 4];
        reader.read_exact(&mut nanos)?;
        let nanos = u32::from_le_bytes(nanos);
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::new(secs, nanos);
        let mut data = vec![];
        reader.read_to_end(&mut data)?;
        Ok(Self {
            timestamp,
            data: Cow::Owned(data),
        })
    }
}

async fn run_record(output_dir: &PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(output_dir).await?;
    let time = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = output_dir.join(format!("dump_{}.bin", time));
    let mut file = tokio::fs::File::create(&path).await?;

    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    while let Some(data) = rx.next().await {
        let data = data?;
        let mut buf = vec![];
        let record = DumpRecord::now(&data[..]);
        record.write_to(&mut buf)?;

        use miniz_oxide::deflate::compress_to_vec;
        let compressed = compress_to_vec(&buf, 6);
        let bin = rmp_serde::encode::to_vec(&compressed)?;
        use tokio::io::AsyncWriteExt;
        file.write_all(&bin).await?;
        tx.send(data).await?;
    }
    Ok(())
}

async fn run_replay(input_file: &PathBuf) -> Result<()> {
    let file = std::fs::File::open(input_file)?;
    let mut file = std::io::BufReader::new(file);

    let mut replay_time_offset = None;

    let (mut tx, rx) = kble_socket::from_stdio().await;
    tokio::spawn(/* just consume everything */ rx.count());
    while let Ok(compressed) = rmp_serde::decode::from_read::<_, Vec<u8>>(&mut file) {
        let decompressed =
            miniz_oxide::inflate::decompress_to_vec(&compressed).map_err(|e| anyhow::anyhow!(e))?;
        let record = DumpRecord::read_from(&mut decompressed.as_slice())?;
        let replay_time_offset = match replay_time_offset {
            Some(offset) => offset,
            None => {
                let offset_value = std::time::SystemTime::now().duration_since(record.timestamp)?;
                replay_time_offset = Some(offset_value);
                offset_value
            }
        };
        let replay_time = record.timestamp + replay_time_offset;
        let sleep_time = replay_time.duration_since(std::time::SystemTime::now())?;
        tokio::time::sleep(sleep_time).await;
        tx.send(record.data.into_owned().into()).await?;
    }

    Ok(())
}
