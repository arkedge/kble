use std::pin::Pin;

use anyhow::Result;

use futures_util::{Sink, Stream};

pub type SocketSink = Pin<Box<dyn Sink<Vec<u8>, Error = anyhow::Error> + Send + 'static>>;
pub type SocketStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send + 'static>>;

#[cfg(feature = "stdio")]
mod stdio;
#[cfg(feature = "stdio")]
pub use stdio::{from_stdio, Stdio};

#[cfg(feature = "tungstenite")]
mod tungstenite;
#[cfg(feature = "tungstenite")]
pub use tungstenite::from_tungstenite;

#[cfg(feature = "axum")]
mod axum;
#[cfg(feature = "axum")]
pub use crate::axum::from_axum;
