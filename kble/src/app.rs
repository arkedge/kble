use crate::{
    plug,
    spaghetti::{Config, Validated},
};
use anyhow::{Context, Result};
use futures::{future, SinkExt, StreamExt};
use std::collections::HashMap;
use tokio::sync::broadcast;
use tracing::{debug, trace, warn};

struct Connection {
    backend: plug::Backend,
    stream: Option<plug::PlugStream>,
    sink: Option<plug::PlugSink>,
}

struct Connections<'a> {
    // Some: connections not used yet
    // None: connections is used in a link
    map: HashMap<&'a str, Connection>,
    termination_grace_period_secs: u64,
}

struct Link<'a> {
    source_name: &'a str,
    dest_name: &'a str,
    source: plug::PlugStream,
    dest: plug::PlugSink,
}

pub async fn run(config: &Config, termination_grace_period_secs: u64) -> Result<()> {
    let mut conns = connect_to_plugs(config, termination_grace_period_secs).await?;
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

    for link in links {
        conns.return_link(link);
    }
    conns.close_and_wait().await?;

    Ok(())
}

impl<'a> Connections<'a> {
    fn new(termination_grace_period_secs: u64) -> Self {
        Self {
            map: HashMap::new(),
            termination_grace_period_secs,
        }
    }

    fn insert(
        &mut self,
        name: &'a str,
        backend: plug::Backend,
        stream: plug::PlugStream,
        sink: plug::PlugSink,
    ) {
        self.map.insert(
            name,
            Connection {
                backend,
                stream: Some(stream),
                sink: Some(sink),
            },
        );
    }

    fn return_link(&mut self, link: Link<'a>) {
        let conn = self.map.get_mut(link.source_name).unwrap_or_else(|| {
            panic!(
                "tried to return a invalid link with source name {}",
                link.source_name,
            )
        });
        conn.stream = Some(link.source);

        let conn = self.map.get_mut(link.dest_name).unwrap_or_else(|| {
            panic!(
                "tried to return a invalid link with dest name {}",
                link.dest_name,
            )
        });
        conn.sink = Some(link.dest);
    }

    // close all connections
    // assume all links are returned
    async fn close_and_wait(self) -> Result<()> {
        let futs = self.map.into_iter().map(|(name, mut conn)| async move {
            let fut = async {
                if let Some(mut s) = conn.sink {
                    debug!("Closing {name}");
                    s.close().await?;
                    debug!("Closed {name}");
                }
                debug!("Waiting for plug {name} to exit");
                conn.backend.wait().await?;
                debug!("Plug {name} exited");
                anyhow::Ok(())
            };
            let close_result = tokio::time::timeout(
                std::time::Duration::from_secs(self.termination_grace_period_secs),
                fut,
            )
            .await;

            match close_result {
                Ok(result) => result,
                Err(_) => {
                    // abandon the connection
                    warn!("Plug {name} didn't exit in time");
                    conn.backend.kill().await?;
                    Ok(())
                }
            }
        });
        future::try_join_all(futs).await?;
        Ok(())
    }

    fn take_stream(&mut self, name: &str) -> Option<plug::PlugStream> {
        self.map.get_mut(name)?.stream.take()
    }

    fn take_sink(&mut self, name: &str) -> Option<plug::PlugSink> {
        self.map.get_mut(name)?.sink.take()
    }
}

async fn connect_to_plugs(
    config: &Config,
    termination_grace_period_secs: u64,
) -> Result<Connections> {
    let mut conns = Connections::new(termination_grace_period_secs);
    for (name, url) in config.plugs().iter() {
        debug!("Connecting to {name}");
        let connect_result = plug::connect(url).await.with_context(move || {
            format! {
                "Failed to connect to plug `{name}`"
            }
        });

        let (backend, sink, stream) = match connect_result {
            Ok(p) => p,
            Err(e) => {
                warn!("Error connecting to {name}: {e}");
                conns.close_and_wait().await?;
                return Err(e);
            }
        };
        debug!("Connected to {name}");
        conns.insert(name.as_str(), backend, stream, sink);
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

            let data_len = data.len();
            if let Err(e) = self.dest.send(data).await {
                warn!("Error writing to {}: {}", self.dest_name, e);
                break;
            }
            trace!(
                "{} -> {}: {} bytes",
                self.source_name,
                self.dest_name,
                data_len
            );
        }
        self
    }
}
