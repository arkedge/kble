use crate::{plug, spaghetti::Config};
use anyhow::{anyhow, Result};
use futures::future;
use futures::StreamExt;
use std::collections::HashMap;

pub async fn run(config: &Config) -> Result<()> {
    let mut sinks = HashMap::new();
    let mut streams = HashMap::new();
    for (name, url) in config.plugs.iter() {
        let (sink, stream) = plug::connect(url).await?;
        sinks.insert(name.as_str(), sink);
        streams.insert(name.as_str(), stream);
    }
    let mut edges = vec![];
    for (stream_name, sink_name) in config.links.iter() {
        let Some(stream) = streams.remove(stream_name.as_str()) else {
            return Err(anyhow!("No such plug: {stream_name}"));
        };
        let Some(sink) = sinks.remove(sink_name.as_str()) else {
            return Err(anyhow!("No such plug or already used: {sink_name}"));
        };
        let edge = stream.forward(sink);
        edges.push(edge);
    }
    future::try_join_all(edges).await?;
    Ok(())
}
