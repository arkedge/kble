use crate::{
    plug,
    spaghetti::{Config, Validated},
};
use anyhow::{Context, Result};
use futures::{future, SinkExt, StreamExt};
use std::collections::HashMap;
use tokio::sync::broadcast;
use tracing::{debug, warn};

struct Connections<'a> {
    // Some: connections not used yet
    // None: connections is used in a link
    map: HashMap<&'a str, (Option<plug::PlugStream>, Option<plug::PlugSink>)>,
}

struct Link<'a> {
    source_name: &'a str,
    dest_name: &'a str,
    source: plug::PlugStream,
    dest: plug::PlugSink,
}

pub async fn run(config: &Config) -> Result<()> {
    let mut conns = connect_to_plugs(config).await?;
    let links = connect_links(&mut conns, config);

    let (quit_tx, _) = broadcast::channel(1);
    let link_futs = links.map(|link| {
        let quit_rx = quit_tx.subscribe();
        let fut = link.forward(quit_rx);
        Box::pin(fut)
    });

    let (terminated_link, _, link_futs) = futures::future::select_all(link_futs).await;
    quit_tx.send(())?;
    let links = future::join_all(link_futs).await;
    let links = links.into_iter().chain(std::iter::once(terminated_link));

    let link_close_futs = future::try_join_all(links.map(|link| link.close()));
    future::try_join(conns.close_all(), link_close_futs).await?;

    Ok(())
}

impl<'a> Connections<'a> {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn insert(&mut self, name: &'a str, stream: plug::PlugStream, sink: plug::PlugSink) {
        self.map.insert(name, (Some(stream), Some(sink)));
    }

    // close all connections whose sink is not used in a link
    async fn close_all(self) -> Result<()> {
        let futs = self.map.into_iter().map(|(name, (_, sink))| async move {
            if let Some(mut s) = sink {
                debug!("Closing {name}");
                s.close().await?;
                debug!("Closed {name}");
            }
            anyhow::Ok(())
        });
        future::try_join_all(futs).await?;
        Ok(())
    }

    fn take_stream(&mut self, name: &str) -> Option<plug::PlugStream> {
        self.map.get_mut(name)?.0.take()
    }

    fn take_sink(&mut self, name: &str) -> Option<plug::PlugSink> {
        self.map.get_mut(name)?.1.take()
    }
}

async fn connect_to_plugs(config: &Config) -> Result<Connections> {
    let mut conns = Connections::new();
    for (name, url) in config.plugs().iter() {
        debug!("Connecting to {name}");
        let connect_result = plug::connect(url).await.with_context(move || {
            format! {
                "Failed to connect to plug `{name}`"
            }
        });

        let (sink, stream) = match connect_result {
            Ok(p) => p,
            Err(e) => {
                warn!("Error connecting to {name}: {e}");
                conns.close_all().await?;
                return Err(e);
            }
        };
        debug!("Connected to {name}");
        conns.insert(name.as_str(), stream, sink);
    }
    Ok(conns)
}

fn connect_links<'a, 'conns>(
    conns: &'conns mut Connections<'a>,
    config: &'a Config<Validated>, // emphasize that the config is validated
) -> impl Iterator<Item = Link<'a>> + 'conns {
    config.links().iter().map(|(source_name, dest_name)| {
        // Those panics shouldn't happen if config is valid and conns is properly initialized
        let source = conns.take_stream(source_name).unwrap_or_else(|| {
            panic!("stream not found: {source_name}");
        });
        let dest = conns.take_sink(dest_name).unwrap_or_else(|| {
            panic!("sink not found: {dest_name}");
        });

        Link {
            source_name,
            dest_name,
            source,
            dest,
        }
    })
}

impl<'a> Link<'a> {
    async fn forward(mut self, mut quit_rx: broadcast::Receiver<()>) -> Self {
        loop {
            let recv_result = tokio::select! {
                _ = quit_rx.recv() => break,
                recv_result = self.source.next() => match recv_result {
                    Some(data) => data,
                    None => break,
                },
            };

            let data = match recv_result {
                Err(e) => {
                    warn!("Error reading from {}: {}", self.source_name, e);
                    break;
                }
                Ok(data) => data,
            };

            if let Err(e) = self.dest.send(data).await {
                warn!("Error writing to {}: {}", self.dest_name, e);
                break;
            }
        }
        self
    }

    async fn close(mut self) -> Result<()> {
        debug!("Closing {}", self.dest_name);
        self.dest.close().await?;
        debug!("Closed {}", self.dest_name);
        Ok(())
    }
}
