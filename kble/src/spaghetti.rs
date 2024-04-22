use anyhow::{anyhow, Result};
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Inner {
    plugs: HashMap<String, Url>,
    links: HashMap<String, String>,
}

#[derive(PartialEq, Debug)]
pub enum Raw {}
pub enum Validated {}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Config<State = Validated> {
    #[serde(flatten)]
    inner: Inner,
    state: std::marker::PhantomData<State>,
}

impl<'de> serde::Deserialize<'de> for Config<Raw> {
    fn deserialize<D>(deserializer: D) -> Result<Config<Raw>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let inner = Inner::deserialize(deserializer)?;
        Ok(Config::new(inner))
    }
}

impl<State> Config<State> {
    fn new(inner: Inner) -> Self {
        Config {
            inner,
            state: std::marker::PhantomData,
        }
    }
}

impl Config<Raw> {
    pub fn validate(self) -> Result<Config<Validated>> {
        use std::collections::HashSet;
        let mut seen_sinks = HashSet::new();

        for (stream_name, sink_name) in self.inner.links.iter() {
            if !self.inner.plugs.contains_key(stream_name) {
                return Err(anyhow!("No such plug: {stream_name}"));
            }
            if !self.inner.plugs.contains_key(sink_name) {
                return Err(anyhow!("No such plug: {sink_name}"));
            }

            if seen_sinks.contains(sink_name) {
                return Err(anyhow!("Sink {sink_name} used more than once"));
            }
            seen_sinks.insert(sink_name);
        }
        Ok(Config::new(self.inner))
    }
}

impl Config<Validated> {
    pub fn plugs(&self) -> &HashMap<String, Url> {
        &self.inner.plugs
    }

    pub fn links(&self) -> &HashMap<String, String> {
        &self.inner.links
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use url::Url;

    #[test]
    fn test_de() {
        let yaml = "plugs:\n  tfsync: exec:tfsync foo\n  seriald: ws://seriald.local/\nlinks:\n  tfsync: seriald\n";
        let inner = Inner {
            plugs: HashMap::from_iter([
                ("tfsync".to_string(), Url::parse("exec:tfsync foo").unwrap()),
                (
                    "seriald".to_string(),
                    Url::parse("ws://seriald.local").unwrap(),
                ),
            ]),
            links: HashMap::from_iter([("tfsync".to_string(), "seriald".to_string())]),
        };
        let expected = Config {
            inner,
            state: std::marker::PhantomData,
        };
        let actual = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(expected, actual);
        actual.validate().unwrap();
    }

    #[test]
    fn test_de_invalid_dest() {
        let yaml = "plugs:\n  tfsync: exec:tfsync foo\n  seriald: ws://seriald.local/\nlinks:\n  tfsync: serialdxxxx\n";
        let actual: Config<Raw> = serde_yaml::from_str(yaml).unwrap();
        assert!(actual.validate().is_err());
    }

    #[test]
    fn test_de_invalid_source() {
        let yaml = "plugs:\n  tfsync: exec:tfsync foo\n  seriald: ws://seriald.local/\nlinks:\n  tfsyncxxxx: seriald\n";
        let actual: Config<Raw> = serde_yaml::from_str(yaml).unwrap();
        assert!(actual.validate().is_err());
    }

    #[test]
    fn test_de_duplicate_sink() {
        let yaml = "plugs:\n  tfsync: exec:tfsync foo\n  seriald: ws://seriald.local/\nlinks:\n  tfsync: seriald\n  seriald: seriald\n";
        let actual: Config<Raw> = serde_yaml::from_str(yaml).unwrap();
        assert!(actual.validate().is_err());
    }
}
