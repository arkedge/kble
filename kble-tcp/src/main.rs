use std::net::SocketAddr;

use anyhow::Result;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    addr: SocketAddr,
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

    let tcp_stream = TcpStream::connect(args.addr).await?;
    let (mut tcp_upstream, mut tcp_downstream) = tokio::io::split(tcp_stream);
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let to_tcp = async {
        while let Some(body) = rx.next().await {
            let body = body?;
            tcp_downstream.write_all(&body).await?;
        }
        anyhow::Ok(())
    };
    let from_tcp = async {
        let mut buffer = [0; 8192];
        loop {
            match tcp_upstream.read(&mut buffer).await? {
                0 => break,
                n => {
                    tx.send(buffer[..n].to_vec().into()).await?;
                }
            }
        }
        anyhow::Ok(())
    };

    let result = tokio::select! {
        // Deterministic winner if both sides close in the same poll: the exit
        // code and any logged error are then reproducible.
        biased;
        r = to_tcp => r,
        r = from_tcp => r,
    };
    if let Err(e) = &result {
        tracing::error!("bridge terminated with error: {e}");
    }

    // `kble_socket::from_stdio` reads via `tokio::io::stdin()`, whose blocking
    // read thread cannot be cancelled mid-read. The bridge can legitimately
    // finish via its TCP side (`from_tcp` hits EOF) while stdin is still open;
    // returning to the runtime would then block shutdown on that stuck thread,
    // so a closed TCP peer would never terminate us (and our WS output would
    // stay open, hiding the disconnect from the orchestrator). Exit explicitly
    // so either side closing promptly ends the process.
    std::process::exit(i32::from(result.is_err()))
}
