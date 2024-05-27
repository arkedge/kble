use anyhow::Result;
use futures::{SinkExt, StreamExt};
use std::env::args;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = args().collect();
    let addr = format!("{}:{}", args.get(1).unwrap(), args.get(2).unwrap());

    let tcp_stream = TcpStream::connect(addr).await?;
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
            match tcp_upstream.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    tx.send(buffer[..n].to_vec().into()).await?;
                }
                Err(e) => {
                    eprintln!("failed to read from socket; error = {:?}", e);
                    break;
                }
            }
        }
        anyhow::Ok(())
    };

    tokio::select! {
        _ = to_tcp => Ok(()),
        _ = from_tcp => Ok(())
    }
}
