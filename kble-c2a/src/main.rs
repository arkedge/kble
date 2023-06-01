use anyhow::Result;
use bytes::BytesMut;
use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use kble_c2a::tfsync;
use tokio_util::codec::Decoder;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Split contiguous AOS Transfer Frames into each frame heuristically
    Tfsync,
}

pub async fn run_tfsync() -> Result<()> {
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let mut buf = BytesMut::new();
    let mut codec = tfsync::AosTransferFrameCodec::new();
    loop {
        let Some(chunk) = rx.next().await else {
            break;
        };
        buf.extend_from_slice(&chunk?);
        while let Some(frame) = codec.decode(&mut buf)? {
            tx.send(frame).await?;
        }
    }
    Ok(())
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

    let args = Args::parse();
    match args.command {
        Commands::Tfsync => run_tfsync().await,
    }
}
