use std::pin::Pin;

use anyhow::Result;
use bytes::Bytes;
use futures_util::{Sink, Stream};

pub type SocketSink = Pin<Box<dyn Sink<Bytes, Error = anyhow::Error> + Send + 'static>>;
pub type SocketStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + 'static>>;

#[cfg(feature = "stdio")]
mod stdio;
#[cfg(feature = "stdio")]
pub use stdio::Stdio;
// `from_stdio` builds a WebSocket, so it also needs `tungstenite`. Gate the
// re-export on both features so `features = ["stdio"]` alone still compiles
// (giving the `Stdio` byte-stream adapter without `from_stdio`).
#[cfg(all(feature = "stdio", feature = "tungstenite"))]
pub use stdio::from_stdio;

#[cfg(feature = "tungstenite")]
mod tungstenite;
#[cfg(feature = "tungstenite")]
pub use tungstenite::from_tungstenite;

#[cfg(feature = "axum")]
mod axum;
#[cfg(feature = "axum")]
pub use crate::axum::from_axum;
