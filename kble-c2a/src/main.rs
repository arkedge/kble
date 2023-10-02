use anyhow::Result;
use bytes::BytesMut;
use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use kble_c2a::{spacepacket, tfsync};
use license::ParseWithLicenseExt;
use tokio_util::codec::Decoder;
use tracing_subscriber::{prelude::*, EnvFilter};

mod license;

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
    /// Manipulate Space Packets
    Spacepacket {
        #[clap(subcommand)]
        command: Spacepacket,
    },
}

#[derive(Subcommand, Debug)]
enum Spacepacket {
    /// Unwrap Space Packet from TC Transfer Frame (adhoc)
    FromTcTf,
    /// Wrap Space Packet with AOS Transfer Frame (adhoc)
    ToAosTf,
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

    let args = Args::parse_with_license();
    match args.command {
        Commands::Tfsync => run_tfsync().await,
        Commands::Spacepacket { command } => run_spacepacket(command).await,
    }
}

async fn run_tfsync() -> Result<()> {
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

async fn run_spacepacket(command: Spacepacket) -> Result<()> {
    match command {
        Spacepacket::FromTcTf => run_sp_from_tc_tf().await,
        Spacepacket::ToAosTf => run_sp_to_aos_tf().await,
    }
}

async fn run_sp_from_tc_tf() -> Result<()> {
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    loop {
        let Some(tc_tf) = rx.next().await else {
            break;
        };
        let spacepacket = spacepacket::from_tc_tf(tc_tf?)?;
        tx.send(spacepacket).await?;
    }
    Ok(())
}

async fn run_sp_to_aos_tf() -> Result<()> {
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let mut frame_count = 0;
    loop {
        let Some(spacepacket) = rx.next().await else {
            break;
        };
        let aos_tf = spacepacket::to_aos_tf(&mut frame_count, spacepacket?)?;
        tx.send(aos_tf.freeze()).await?;
    }
    Ok(())
}
