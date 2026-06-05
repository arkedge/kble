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

    // Either side closing ends the bridge; `biased` makes the winner
    // deterministic when both close in the same poll. With the piped stdin/stdout
    // a plug has under the orchestrator, `kble_socket::from_stdio` is cancellable,
    // so returning here lets the runtime shut down cleanly. Any `Err` is reported
    // by `main`'s `Termination` impl.
    tokio::select! {
        biased;
        r = to_tcp => r,
        r = from_tcp => r,
    }
}
