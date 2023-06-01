use anyhow::Result;
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use kble_tfsync::AosTransferFrameCodec;
use tokio_util::codec::Decoder;
use tracing_subscriber::{prelude::*, EnvFilter};

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
    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let mut buf = BytesMut::new();
    let mut codec = AosTransferFrameCodec::new();
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
