use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub plugs: HashMap<String, Url>,
    pub links: HashMap<String, String>,
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
