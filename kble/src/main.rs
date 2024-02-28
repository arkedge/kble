use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use notalawyer_clap::*;

mod plug;
mod spaghetti;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, short)]
    spaghetti: PathBuf,
}

impl Args {
    fn load_spaghetti_config(&self) -> Result<spaghetti::Config> {
        let spaghetti_file = std::fs::OpenOptions::new()
            .read(true)
            .open(&self.spaghetti)
            .with_context(|| format!("Failed to open {:?}", &self.spaghetti))?;
        let spagetthi_rdr = std::io::BufReader::new(spaghetti_file);
        serde_yaml::from_reader(spagetthi_rdr)
            .with_context(|| format!("Unable to parse {:?}", self.spaghetti))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse_with_license_notice(include_notice!());
    let config = args.load_spaghetti_config()?;
    config.run().await?;
    Ok(())
}
