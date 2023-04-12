use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::Result;
use axum::{
    extract::{ws::WebSocket, Query, WebSocketUpgrade},
    http::StatusCode,
    response::Response,
    routing::get,
    Router,
};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use kble_socket::from_axum;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{DataBits, FlowControl, Parity, SerialPortBuilderExt, SerialStream, StopBits};
use tracing::error;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Debug, Deserialize)]
#[serde(remote = "DataBits")]
pub enum DataBitsDef {
    #[serde(rename = "5")]
    Five,
    #[serde(rename = "6")]
    Six,
    #[serde(rename = "7")]
    Seven,
    #[serde(rename = "8")]
    Eight,
}

#[derive(Debug, Deserialize)]
#[serde(remote = "FlowControl")]
#[serde(rename_all = "snake_case")]
pub enum FlowControlDef {
    None,
    Software,
    Hardware,
}

#[derive(Debug, Deserialize)]
#[serde(remote = "Parity")]
#[serde(rename_all = "snake_case")]
pub enum ParityDef {
    None,
    Even,
    Odd,
}

#[derive(Debug, Deserialize)]
#[serde(remote = "StopBits")]
#[serde(rename_all = "snake_case")]
pub enum StopBitsDef {
    #[serde(rename = "1")]
    One,
    #[serde(rename = "2")]
    Two,
}

#[derive(Debug, Deserialize)]
pub struct SerialPortOptions {
    port: String,
    baudrate: u32,
    #[serde(with = "DataBitsDef", default = "databits_default")]
    databits: DataBits,
    #[serde(with = "FlowControlDef", default = "flowcontrol_default")]
    flowcontrol: FlowControl,
    #[serde(with = "ParityDef", default = "parity_default")]
    parity: Parity,
    #[serde(with = "StopBitsDef", default = "stopbits_default")]
    stopbits: StopBits,
}

fn databits_default() -> DataBits {
    DataBits::Eight
}

fn flowcontrol_default() -> FlowControl {
    FlowControl::None
}

fn parity_default() -> Parity {
    Parity::None
}

fn stopbits_default() -> StopBits {
    StopBits::One
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(action, long, env, default_value_t = Ipv4Addr::UNSPECIFIED.into())]
    addr: IpAddr,
    #[clap(action, long, env, default_value_t = 9600)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr),
        )
        .with(EnvFilter::from_default_env())
        .init();

    let app = Router::new().route("/open", get(handle_get));
    let addr = SocketAddr::new(args.addr, args.port);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

async fn handle_get(
    upgrade: WebSocketUpgrade,
    opts: Query<SerialPortOptions>,
) -> Result<Response, StatusCode> {
    let serialport = tokio_serial::new(&opts.port, opts.baudrate)
        .data_bits(opts.databits)
        .flow_control(opts.flowcontrol)
        .parity(opts.parity)
        .stop_bits(opts.stopbits)
        .open_native_async()
        .map_err(|err| {
            error!("{:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(upgrade.on_upgrade(|ws| handle_ws(ws, serialport)))
}

async fn handle_ws(ws: WebSocket, serialport: SerialStream) {
    let (mut sink, mut stream) = from_axum(ws);
    let (mut rx, mut tx) = tokio::io::split(serialport);
    let rx_fut = async {
        loop {
            let mut buf = vec![0u8; 4096];
            let len = rx.read(&mut buf).await?;
            if len == 0 {
                break;
            }
            buf.truncate(len);
            sink.send(buf).await?;
        }
        anyhow::Ok(())
    };
    let tx_fut = async {
        loop {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let chunk = chunk?;
            tx.write_all(&chunk).await?;
        }
        anyhow::Ok(())
    };
    futures::future::try_join(rx_fut, tx_fut).await.ok();
}
