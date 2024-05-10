use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use notalawyer_clap::*;
use tracing_subscriber::{prelude::*, EnvFilter};

mod app;
mod plug;
mod spaghetti;

use spaghetti::{Config, Raw};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, short)]
    spaghetti: PathBuf,

    /// Period to wait for each child process to exit after a closing handshake
    /// before killing it
    #[clap(long, default_value_t = 10)]
    termination_grace_period_secs: u64,
}

impl Args {
    fn load_spaghetti_config(&self) -> Result<spaghetti::Config> {
        let spaghetti_file = std::fs::OpenOptions::new()
            .read(true)
            .open(&self.spaghetti)
            .with_context(|| format!("Failed to open {:?}", &self.spaghetti))?;
        let spagetthi_rdr = std::io::BufReader::new(spaghetti_file);
        let raw: Config<Raw> = serde_yaml::from_reader(spagetthi_rdr)
            .with_context(|| format!("Unable to parse {:?}", self.spaghetti))?;
        raw.validate()
            .with_context(|| format!("Invalid configuration in {:?}", self.spaghetti))
    }
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
    tracing::info!("Starting");

    let args = Args::parse_with_license_notice(include_notice!());
    let config = args.load_spaghetti_config()?;
    app::run(&config, args.termination_grace_period_secs).await?;
    Ok(())
}
