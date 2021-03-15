use color_eyre::eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use structopt::StructOpt;
use tokio::sync::broadcast;

use crate::{
    config::{Container, TimeDuration},
    container::container_spawner,
};

mod config;
#[allow(dead_code)]
mod container;

#[derive(Debug, Serialize, Deserialize)]
struct Configuration {
    time: TimeDuration,
    containers: Vec<Container>,
}

#[derive(StructOpt)]
struct SpawnnerOpts {
    config: PathBuf,
}

async fn deserialize_config(path: &Path) -> Result<Configuration> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path)
            .wrap_err("Failed to open input file")
            .and_then(|file| {
                serde_yaml::from_reader(file).wrap_err("Failed to deserialize input file")
            })
    })
    .await
    .wrap_err("Failed to join blokcing task")?
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let opts = SpawnnerOpts::from_args();
    let config = deserialize_config(&opts.config).await?;

    // setup logging
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()?;

    let (stop, _) = broadcast::channel::<u8>(1);
    let spawners = config
        .containers
        .into_iter()
        .map(|s| {
            let mut stop_wait = stop.subscribe();
            container_spawner(s, async move { stop_wait.recv().await })
        })
        .map(tokio::spawn)
        .collect::<Vec<_>>();

    // wait duration
    let sleep_duration: Duration = config.time.into();
    log::debug!("Sleeping for {}", sleep_duration.as_secs());
    tokio::time::sleep(config.time.into()).await;
    stop.send(1).unwrap();
    for spawner in spawners {
        spawner.await??;
    }
    Ok(())
}
