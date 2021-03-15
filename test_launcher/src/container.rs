// Copyright (c) 2019 - 2020 ESRLabs
//
//   Licensed under the Apache License, Version 2.0 (the "License");
//   you may not use this file except in compliance with the License.
//   You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
//   Unless required by applicable law or agreed to in writing, software
//   distributed under the License is distributed on an "AS IS" BASIS,
//   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//   See the License for the specific language governing permissions and
//   limitations under the License.

use crate::config::{self};
use color_eyre::eyre::{Result, WrapErr};
use escargot::CargoBuild;
use lazy_static::lazy_static;
use northstar::api::client;
use npk::{manifest::Version, npk::pack};
use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{runtime::Runtime, sync::oneshot};

// compile test_launches
lazy_static! {
    static ref TEST_CONTAINER_BIN: PathBuf = {
        CargoBuild::new()
            .manifest_path("northstar_tests/test_container/Cargo.toml")
            .run()
            .wrap_err("Failed to build the test_container")
            .unwrap()
            .path()
            .to_owned()
    };
}

// TODO
fn gen_script() -> String {
    String::from("echo I'm alive!")
}

fn gen_manifest(name: &str) -> String {
    format!(
        "
name: {}
version: 0.0.1
init: /test_container
uid: 1000
gid: 1000
args:
    - {}
mounts:
  /data: persist
  /lib:
    host: /lib
  /lib64:
    host: /lib64
io:
  stdout:
    log:
      - DEBUG
      - test_container
",
        name, "/input.txt"
    )
}

/// Creates a copy of the test_container in the provided repository
pub(crate) fn add_container<P>(out_dir: P, name: &str) -> Result<()>
where
    P: AsRef<Path>,
{
    let package_dir = TempDir::new().unwrap();
    let root = package_dir.path().join("root");

    // mkdir -p tmp_dir/root/
    std::fs::create_dir_all(&root).unwrap();

    // copy test_container binary to the root folder
    std::fs::copy(TEST_CONTAINER_BIN.as_path(), &root.join("test_container"))?;

    // Generate the containers manifest
    let manifest_path = package_dir.path().join("manifest").with_extension("yaml");
    std::fs::write(&manifest_path, &gen_manifest(name))?;

    // Generate container input script
    let script = root.join("input").with_extension("txt");
    std::fs::write(&script, &gen_script())?;

    let rt = Runtime::new().wrap_err("Starting a runtime just to pack the container")?;
    rt.block_on(pack(
        &manifest_path,
        package_dir.path().join("root").as_path(),
        out_dir.as_ref(),
        Some(Path::new("examples/keys/northstar.key")),
    ))
    .wrap_err("Failed to pack container")
}

/// Represents a task that spawns a container in determined intervals
pub(crate) struct ContainerTask(Option<oneshot::Sender<u8>>);

impl ContainerTask {
    pub(crate) fn new(rt: &tokio::runtime::Runtime, container: config::Container) -> ContainerTask {
        let (stop, stop_wait) = oneshot::channel();
        rt.spawn(container_spawner(container, stop_wait));
        ContainerTask(Some(stop))
    }

    pub(crate) fn stop(&mut self) {
        if let Some(stop) = self.0.take() {
            stop.send(1).unwrap();
        }
    }
}

/// A task that starts and stops the container in the configured intervals
pub(crate) async fn container_spawner<S>(container: config::Container, stop_wait: S) -> Result<()>
where
    S: Future + 'static,
{
    let mut start_after = tokio::time::interval(container.start_after.into());
    let mut stop_after = tokio::time::interval(container.stop_after.into());
    let name = &container.name;
    let version = &container.version;

    let start_stop_loop = async {
        loop {
            start_after.tick().await;
            start(name, version).await;
            stop_after.tick().await;
            stop(name, version).await;
        }
    };

    tokio::select! {
        _ = start_stop_loop => (),
        _ = stop_wait => (),
    };

    Ok(())
}

/// Open a connection with Northstar
async fn open_connection() -> Option<client::Client> {
    match client::Client::new("tcp://localhost:4200")
        .await
        .wrap_err("Failed to open new client")
    {
        Ok(client) => Some(client),
        Err(e) => {
            log::error!("{}", e);
            None
        }
    }
}

/// Instantiates a client that sends a start request for the container
async fn start(name: &str, version: &Version) {
    log::trace!("Starting container {}", name);
    if let Some(client) = open_connection().await {
        if let Err(e) = client.start(name, version).await {
            log::error!("{}", e);
        }
    }
}

/// Instantiates a client that sends a stop request for the container
async fn stop(name: &str, version: &Version) {
    log::trace!("Stopping container {}", name);
    if let Some(client) = open_connection().await {
        if let Err(e) = client.stop(name, version, Duration::from_secs(10)).await {
            log::error!("{}", e);
        }
    }
}
