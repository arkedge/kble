use std::collections::VecDeque;

use anyhow::Result;
use bytes::BytesMut;
use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use notalawyer_clap::*;
use tokio_util::codec::{Decoder, Encoder};
use tracing::warn;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Encode,
    Decode {
        #[clap(long, short, default_value_t = 2048)]
        buffer_size: usize,
    },
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
        Commands::Encode => run_encode().await,
        Commands::Decode { buffer_size } => run_decode(buffer_size).await,
    }
}

async fn run_encode() -> Result<()> {
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let mut codec = eb90::Encoder::new();
    loop {
        let Some(body) = rx.next().await else {
            break;
        };
        let mut buf = BytesMut::new();
        codec.encode(body?, &mut buf)?;
        tx.send(buf.freeze()).await?;
    }
    Ok(())
}

async fn run_decode(buffer_size: usize) -> Result<()> {
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let mut buf: BytesMut = BytesMut::new();
    let mut codec = eb90::Decoder::new(VecDeque::with_capacity(buffer_size));
    loop {
        let Some(chunk) = rx.next().await else {
            break;
        };
        buf.extend_from_slice(&chunk?);
        while let Some(decoded) = codec.decode(&mut buf)? {
            use eb90::codec::Decoded;
            match decoded {
                Decoded::Frame(frame) => tx.send(frame).await?,
                Decoded::Junk(kind) => warn!(?kind, "received junk data"),
            }
        }
    }
    Ok(())
}
