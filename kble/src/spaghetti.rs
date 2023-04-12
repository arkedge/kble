use std::collections::HashMap;

use anyhow::{anyhow, Result};
use futures::{future, StreamExt};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::plug;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Config {
    plugs: HashMap<String, Url>,
    links: HashMap<String, String>,
}

impl Config {
    pub async fn run(&self) -> Result<()> {
        let mut sinks = HashMap::new();
        let mut streams = HashMap::new();
        for (name, url) in self.plugs.iter() {
            let (sink, stream) = plug::connect(url).await?;
            sinks.insert(name.as_str(), sink);
            streams.insert(name.as_str(), stream);
        }
        let mut edges = vec![];
        for (stream_name, sink_name) in self.links.iter() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use url::Url;

    #[test]
    fn test_de() {
        let yaml = "plugs:\n  tfsync: exec:tfsync foo\n  seriald: ws://seriald.local/\nlinks:\n  tfsync: seriald\n";
        let expected = Config {
            plugs: HashMap::from_iter([
                ("tfsync".to_string(), Url::parse("exec:tfsync foo").unwrap()),
                (
                    "seriald".to_string(),
                    Url::parse("ws://seriald.local").unwrap(),
                ),
            ]),
            links: HashMap::from_iter([("tfsync".to_string(), "seriald".to_string())]),
        };
        let actual = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(expected, actual);
    }
}
