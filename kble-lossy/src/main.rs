use anyhow::Result;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use notalawyer_clap::*;
use rand::{Rng, SeedableRng};
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "-1.0")]
    loss_rate: f32,
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

    let (mut tx, mut rx) = kble_socket::from_stdio().await;
    let mut small_rng = rand::rngs::SmallRng::from_entropy();

    while let Some(data) = rx.next().await {
        let data = data?;

        if small_rng.gen::<f32>() < args.loss_rate {
            continue;
        }
        tx.send(data).await?
    }

    Ok(())
}
