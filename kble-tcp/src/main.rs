use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::net::{IpAddr, Ipv4Addr};
use std::process::exit;
use std::sync::Arc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::protocol::Message;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, env, default_value_t = Ipv4Addr::UNSPECIFIED.into())]
    ws_addr: IpAddr,
    #[clap(long, env, default_value_t = 15001)]
    ws_port: u16,
    #[clap(long, env)]
    tcp_addr: IpAddr,
    #[clap(long, env)]
    tcp_port: u16,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    // WebSocketサーバーのリスナーを設定
    let ws_listener = TcpListener::bind(format!("{}:{}", args.ws_addr, args.ws_port)).await?;
    println!(
        "WebSocket server listening on ws://{}:{}",
        args.ws_addr, args.ws_port
    );

    // 接続状態を追跡するためのArc<Mutex<>>を作成
    let client_connected = Arc::new(Mutex::new(false));

    // TCPサーバーに接続する
    let tcp_stream = TcpStream::connect(format!("{}:{}", args.tcp_addr, args.tcp_port)).await?;
    let (tcp_upstream, tcp_downstream) = tokio::io::split(tcp_stream);
    let tcp_upstream = Arc::new(Mutex::new(tcp_upstream));
    let tcp_downstream = Arc::new(Mutex::new(tcp_downstream));

    loop {
        let (stream, _) = ws_listener
            .accept()
            .await
            .expect("Failed to accept connection");

        let client_connected = Arc::clone(&client_connected);
        let tcp_upstream = Arc::clone(&tcp_upstream);
        let tcp_downstream = Arc::clone(&tcp_downstream);

        tokio::spawn(async move {
            let mut is_client_connected = client_connected.lock().await;
            if *is_client_connected {
                println!("Already connected, rejecting new connection");
            } else {
                *is_client_connected = true;
                drop(is_client_connected); // Mutexのロックを解除
                handle_ws_client(
                    stream,
                    Arc::clone(&client_connected),
                    Arc::clone(&tcp_upstream),
                    Arc::clone(&tcp_downstream),
                )
                .await;
            }
        });
    }
}

async fn handle_ws_client(
    ws_stream: TcpStream,
    client_connected: Arc<Mutex<bool>>,
    tcp_upstream: Arc<Mutex<tokio::io::ReadHalf<TcpStream>>>,
    tcp_downstream: Arc<Mutex<tokio::io::WriteHalf<TcpStream>>>,
) {
    let ws_stream = accept_async(ws_stream)
        .await
        .expect("Error during WebSocket handshake");
    println!("WebSocket client connected");

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // WebSocket -> TCP (upstream)
    let ws_to_tcp = async {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    println!("Received from WebSocket client: {}", text);
                    let mut tcp_stream = tcp_downstream.lock().await;
                    if tcp_stream.write_all(text.as_bytes()).await.is_err() {
                        println!("Failed to write to TCP server");
                        break;
                    }
                }
                Ok(Message::Binary(bin)) => {
                    println!("Received binary data from WebSocket client");
                    let mut tcp_stream = tcp_downstream.lock().await;
                    if tcp_stream.write_all(&bin).await.is_err() {
                        println!("Failed to write to TCP server");
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    println!("WebSocket client disconnected");
                    break;
                }
                _ => (),
            }
        }
    };

    // TCP -> WebSocket (downstream)
    let tcp_to_ws = async {
        let mut buffer = [0; 8192];
        loop {
            let mut tcp_stream = tcp_upstream.lock().await;
            match tcp_stream.read(&mut buffer).await {
                Ok(0) => {
                    println!("TCP server disconnected");
                    exit(0);
                }
                Ok(n) => {
                    if ws_write
                        .send(Message::binary(buffer[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        println!("Failed to send message to WebSocket client");
                        break;
                    }
                }
                Err(_) => {
                    println!("Failed to read from TCP server");
                    break;
                }
            }
        }
    };

    // 両方のタスクを同時に実行
    tokio::select! {
        _ = ws_to_tcp => (),
        _ = tcp_to_ws => (),
    }

    println!("Connection closed");

    let mut is_client_connected = client_connected.lock().await;
    *is_client_connected = false;
}
