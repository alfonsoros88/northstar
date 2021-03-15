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

use cgroups_rs::cgroup_builder::CgroupBuilder;
use color_eyre::eyre::{Result, WrapErr};
use nix::unistd::{self, fork, ForkResult};
use northstar::api::client::Client;
use serde::{Deserialize, Serialize};
use std::{
    convert::TryInto,
    fs::File,
    io::{BufRead, Write},
    os::unix::io::AsRawFd,
    path::Path,
    time::{Duration, SystemTime},
};
use structopt::StructOpt;

mod config;
mod container;
mod metric;
mod pipe;

fn main() -> Result<()> {
    color_eyre::install()?;
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()?;

    // Command line arguments
    let opt = config::LauncherOpt::from_args();

    // Load configuration
    let config: config::ProgramOpt = {
        serde_yaml::from_reader(
            File::open(&opt.config).wrap_err("Failed to open launcher configuration file")?,
        )?
    };

    log::debug!("Test configuration:\n{}", serde_yaml::to_string(&config)?);
    run(config).wrap_err("Failed to run test")
}

/// This structure holds values that we are interested to retrieve from the "tester" process
#[derive(Debug, Serialize, Deserialize)]
struct TestStats {
    /// This is the duration from the spawn of the northstar runtime till its termination
    duration: Duration,
}

fn run(config: config::ProgramOpt) -> Result<()> {
    //
    // TODO Take system snapshot to compare at the end of the test
    //

    // ## Opening pipes
    //
    // These pipes are for:
    // - Tester stdout
    // - Tester stderr
    // - A pipe for the resulting stats
    //
    log::debug!("Opening pipes for communication with ");
    let (child_out, stdout) = pipe::pipe().wrap_err("Opening stdout pipe")?;
    let (child_err, stderr) = pipe::pipe().wrap_err("Opening stderr pipe")?;
    let (mut child_stats, mut stats) = pipe::pipe().wrap_err("Opening stats pipe")?;

    // ## Create the output directory
    //
    // The name of the directory is a date time and it is created inside the directory specified in
    // `config.out_dir`.
    //
    let datetime = chrono::offset::Utc::now();
    let result_dir = if let Some(entry) = &config.results.entry_name {
        config.results.out_dir.join(&entry)
    } else {
        config
            .results
            .out_dir
            .join(&datetime.format("%d-%m-%Y_%H-%M-%S").to_string())
    };

    log::debug!("Creating output directory {}", result_dir.display());
    std::fs::create_dir_all(&result_dir).wrap_err(format!(
        "Failed to crate output directory: {}",
        result_dir.display()
    ))?;

    // ## Configure the test cgroup.
    //
    // The cgroup is used for:
    //
    // - restrict the test to specific cpus
    // - read for gather metrics:
    //  * memory
    //
    let cgroup = configure_test_cgroup(&config);

    // ## Fork and spawn the process that runs the test
    //
    //  This process (till this point) is the "coordninator" and its job is to gather metrics, wait
    //  for the test results and later store the summary to the output directory.
    //
    //  The "tester" process will set the environment before starting the north runtime.
    //
    log::debug!("Spawnning the Test process");
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child }) => {
            //  ## This is the launcher process
            //
            //  TODO:
            //
            //  * Start redirecting the child stdout and stderr to the directory
            //  * Start the async runtime to handle metrics and io
            //
            //  * collect metrics from the runtime
            //  * connect to the runtime using `nstar`
            //  * trigger some stress
            //  * parse runtime log
            //  * generate the test report
            //
            let tokio = tokio::runtime::Runtime::new().unwrap();

            // ## Capture the child output
            //
            // * Close unused ends of pipes
            // * Write stdout and stderr to separate files in output directory
            //
            drop(stdout);
            drop(stderr);
            drop(stats);

            // ## If set, collect strace output
            //
            // TODO do this properly
            //
            if config.strace {
                let strace_filename = result_dir.join("strace");
                tokio.spawn(strace_child(strace_filename, child));
            }

            // ##  Forward stdio to files
            //
            log::debug!("Pipe stdout and stderr to files");
            let stdout_path = result_dir.join("stdout");
            let stderr_path = result_dir.join("stderr");
            tokio.spawn(async move {
                let mut stdout: pipe::AsyncPipeReader = child_out.try_into()?;
                let mut stderr: pipe::AsyncPipeReader = child_err.try_into()?;
                let (result_out, result_err) = tokio::join!(
                    forward_to_file(&mut stdout, &stdout_path),
                    forward_to_file(&mut stderr, stderr_path),
                );
                result_out.and(result_err)
            });

            // ## Wait for norshtar to be online
            //
            //  TODO find a better way to wait for the console
            tokio.block_on(async {
                loop {
                    if Client::new("tcp://localhost:4200").await.is_ok() {
                        log::debug!("Northstar's console is open");
                        break;
                    }
                }
            });

            // ## Spawn tasks to collect metrics
            //
            //
            let mem_freq = config.metrics.memory.freq;
            let mem_metric = metric::Memory::new(mem_freq.into(), cgroup.clone());
            let mem_csv_filename = result_dir.join("memory.csv");

            let fd_freq = config.metrics.memory.freq;
            let fd_metric = metric::FdCount::new(fd_freq.into(), child.as_raw());
            let fd_csv_filename = result_dir.join("fd.csv");

            let collectors = vec![
                metric::collect_to_csv(&tokio, mem_metric, &mem_csv_filename)?,
                metric::collect_to_csv(&tokio, fd_metric, &fd_csv_filename)?,
            ];

            // ## Start the container launcher task
            //
            //  A task for each container specified in the configuration. Each task will statr and
            //  stop a particular container in intervals.
            //
            let container_tasks = config
                .containers
                .iter()
                .map(|c| container::ContainerTask::new(&tokio, c.clone()))
                .collect::<Vec<_>>();

            // ## Wait for the runtime termination
            match nix::sys::wait::waitpid(child, None) {
                Ok(status) => log::debug!("Tester exit status: {:?}", status),
                Err(e) => log::error!("Waiting on tester failed: {}", e),
            };

            // ## stop container tasks
            for mut container in container_tasks {
                container.stop();
            }

            // ## stop collectors and write remaning stats
            //
            // * collect the metric in csv files
            //
            for collector in collectors {
                if let Err(e) = tokio.block_on(collector.stop()) {
                    log::error!("Failed to wait for collector: {}", e);
                }
            }

            // ## Collect eveything from the dead tester
            //
            // * Get the TestStats
            // * Wait for the stdout and stderr forwarding threads
            // * Collect the metrics
            // * Remove the Cgroup
            //
            let test_stats: TestStats = serde_json::from_reader(&mut child_stats)
                .wrap_err("Failed to get test results from child process")?;

            // ## Take snapshot of memory cgroup and store the result
            //
            {
                let mem_controller: &cgroups_rs::memory::MemController =
                    cgroup.controller_of().unwrap();
                let mut file = std::fs::File::create(result_dir.join("memory_stat"))?;
                writeln!(file, "{:#?}", mem_controller.memory_stat())
                    .wrap_err("Failed to write memory_stat")?;
            }

            // Remove Cgroup
            log::debug!("Remove Cgroup");
            cgroup.delete().wrap_err("Failed to delete test cgroup")?;

            // ## Final part, process the test result:
            //
            //  * Maybe generate some graphs
            //  * Add a README.md with the summary
            //

            // ## Generate graphs
            //
            // - create a directory for plots
            // - plot graphs
            //
            std::fs::create_dir_all(result_dir.join("plots"))
                .wrap_err("Creating plots directory inside report entry")?;

            let memory_graph = result_dir.join("plots/memory.png");
            plot("memory", &mem_csv_filename, &memory_graph)
                .wrap_err("Store the memory graph to plots")?;

            let fd_graph = result_dir.join("plots/fd.png");
            plot("file descriptors", &fd_csv_filename, &fd_graph)
                .wrap_err("Store the fd graph to plots")?;

            // ## Write the summary into the README.md
            //
            // Things to write in the summary:
            // - test duration
            // - revision (maybe as a link)
            // - errors
            // - metrics
            //
            let mut readme = std::fs::File::create(result_dir.join("README.md"))
                .wrap_err("Open README.md to write summary")?;

            // ## Log revision
            //
            let git_output = std::process::Command::new("git")
                .args(&["rev-parse", "HEAD"])
                .output()
                .wrap_err("Failed to get git revision")?;
            let mut revision = String::from_utf8_lossy(&git_output.stdout).into_owned();
            revision.pop();

            log::debug!("Writing summary README.md");
            writeln!(readme, "# Test results ({})\n\n", datetime.to_rfc2822())?;
            writeln!(readme, "Git Revision | Test Duration (seconds)")?;
            writeln!(readme, "------------ | -----------------------")?;
            writeln!(
                readme,
                "northstar@{} | {}\n",
                revision,
                test_stats.duration.as_secs()
            )?;

            writeln!(readme, "### Test configuration")?;
            writeln!(readme, "```yaml")?;
            serde_yaml::to_writer(&mut readme, &config)?;
            writeln!(readme, "\n```")?;

            writeln!(readme)?;
            writeln!(readme, "## Memory consumption\n")?;
            writeln!(readme, "![memory graph](plots/memory.png)")?;
            writeln!(readme, "## Files open\n")?;
            writeln!(readme, "![file descriptors graph](plots/fd.png)")?;
            writeln!(readme, "## Log Errors\n")?;

            // ## Collect errors from log
            // TODO maybe do this during the test run?
            let log = std::fs::File::open(&result_dir.join("stdout"))?;
            let log_reader = std::io::BufReader::new(log);
            let mut log_lines = log_reader.lines();

            struct ContextBuffer(std::collections::VecDeque<String>);
            impl ContextBuffer {
                fn new(size: usize) -> ContextBuffer {
                    ContextBuffer(std::collections::VecDeque::with_capacity(2 * size + 1))
                }

                fn push(&mut self, line: String) {
                    self.0.push_front(line);
                    if self.0.len() == self.0.capacity() {
                        self.0.pop_back();
                    }
                }

                fn get_mid(&self) -> Option<&str> {
                    let mid = self.0.len() / 2;
                    self.0.get(mid).map(|s| s.as_ref())
                }
            }

            let mut context = ContextBuffer::new(config.error_context_lines);
            while let Some(Ok(line)) = log_lines.next() {
                context.push(line);
                if let Some(line) = context.get_mid() {
                    if line.contains("ERROR") {
                        writeln!(readme, "```")?;
                        // print log segment
                        for context_line in context.0.iter().rev() {
                            writeln!(readme, "{}", context_line)?;
                        }
                        writeln!(readme, "```")?;
                    }
                }
            }
        }
        Ok(ForkResult::Child) => {
            //
            //  The spawned process
            //
            //  Preparing the environment
            //
            //  - Assigns itself to the test cgroup
            //  - Remount / with MS_PRIVATE
            //  - Enter in mount namespace
            //
            //

            // ## Add this process to the cgroups
            //
            log::debug!("Adding \"tester\" process to the test cgroup");
            let pid = nix::unistd::getpid().as_raw() as u64;
            cgroup
                .add_task(pid.into())
                .wrap_err("Failed to assign test task to cgroup")?;

            // Set the mount propagation of unshare_root to MS_PRIVATE
            nix::mount::mount(
                Option::<&'static [u8]>::None,
                "/",
                Some("ext4"),
                nix::mount::MsFlags::MS_PRIVATE,
                Option::<&'static [u8]>::None,
            )
            .unwrap();

            // Enter a mount namespace. This needs to be done before spawning
            // the tokio threadpool.
            nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNS).unwrap();

            // Configure tokio runtime
            let tokio = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("long_test")
                .build()?;

            // ## Configure the containers that will be available to the runtime
            //
            // - create "test" repository
            // - add the test containers to the repository
            //
            log::debug!("Creating test repository");
            let test_repo =
                tempfile::TempDir::new().wrap_err("Failed to create test repository")?;
            for container in &config.containers {
                log::debug!("Adding container {} to test repository", container.name);
                container::add_container(&test_repo, &container.name)?;
            }

            // ## Set stdout & stderr
            //
            // - close parent read ends
            // - Set this process stdout and stderr
            //
            log::debug!("Redirecting output in \"tester\" process");
            drop(child_out);
            drop(child_err);
            drop(child_stats);
            unistd::dup2(stdout.as_raw_fd(), 1).wrap_err("Change stdout to out pipe")?;
            unistd::dup2(stderr.as_raw_fd(), 2).wrap_err("Change stderr to err pipe")?;

            // Start northstar
            let duration = config.time;
            let test_start_time = SystemTime::now();
            let northstar_task = async move {
                // Parse northstar configuration
                let config_string = tokio::fs::read_to_string("northstar.toml")
                    .await
                    .wrap_err("Failed to read northstar configuration")?;
                let mut config =
                    toml::from_str::<northstar::runtime::config::Config>(&config_string)
                        .wrap_err("Failed to parse configuration")?;

                // add the test respository
                config.repositories.insert(
                    "test".to_owned(),
                    northstar::runtime::config::Repository {
                        dir: test_repo.path().to_owned(),
                        key: None,
                    },
                );

                // Start northstar
                let mut runtime = northstar::runtime::Runtime::start(config)
                    .await
                    .wrap_err("Failed to start northstar")?;

                // Wait for northstar status
                let status = tokio::select! {
                    _ = tokio::time::sleep(duration.into()) => {
                        log::debug!("Time's up, shutting down the runtime");
                        runtime.shutdown().await
                    }
                    status = &mut runtime => status
                };

                status.wrap_err("Runtime error")
            };

            // TODO use the exit code
            tokio
                .block_on(northstar_task)
                .wrap_err("Failed to stop Northstar without issues")?;

            log::debug!("Test finished");
            let test_duration = test_start_time
                .elapsed()
                .wrap_err("Measuring test duration")?;

            // Send stats to controller
            let test_stats = TestStats {
                duration: test_duration,
            };
            log::debug!("Sending test results back to launcher");
            serde_json::to_writer(&mut stats, &test_stats)?;

            // That's it, bye!
            std::process::exit(0);
        }
        Err(e) => log::error!("Failed to fork: {}", e),
    };
    Ok(())
}

async fn forward_to_file<R, P>(mut reader: &mut R, path: P) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    P: AsRef<Path>,
{
    let mut file = tokio::fs::File::create(path).await?;
    let _ = tokio::io::copy(&mut reader, &mut file).await?;
    Ok(())
}

use plotters::prelude::*;

fn plot<P>(title: &str, csv: P, out: &Path) -> Result<()>
where
    P: AsRef<Path>,
{
    let csv_reader = std::fs::File::open(csv).wrap_err("Opening csv file")?;
    let mut rdr = csv::Reader::from_reader(csv_reader);

    let mut time = Vec::new();
    let mut value = Vec::new();
    for result in rdr.deserialize() {
        let record: metric::MetricRecord<u64> = result.wrap_err("Deserializing record")?;
        time.push(record.timestamp);
        value.push(record.value);
    }

    let (minm, maxm) = (*value.iter().min().unwrap(), *value.iter().max().unwrap());

    let x_spec = {
        let (mint, maxt) = (*time.first().unwrap(), *time.last().unwrap());
        mint..maxt
    };

    let root = BitMapBackend::new(out, (800, 600)).into_drawing_area();

    root.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 50).into_font())
        .margin(5)
        .x_label_area_size(60)
        .y_label_area_size(60)
        .build_cartesian_2d(x_spec, minm..maxm)?;

    fn timestamp_formatter(ts: &chrono::DateTime<chrono::offset::Utc>) -> String {
        ts.format("%H:%M:%S").to_string()
    }

    chart
        .configure_mesh()
        .disable_x_mesh()
        .x_desc("Time")
        .x_label_formatter(&timestamp_formatter)
        .draw()?;

    chart.draw_series(LineSeries::new(time.into_iter().zip(value), &RED))?;
    Ok(())
}

/// Configure the root cgroups for the test
fn configure_test_cgroup(config: &config::ProgramOpt) -> cgroups_rs::Cgroup {
    let hier = cgroups_rs::hierarchies::auto();

    let cpus = config
        .cpus
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<String>>()
        .as_slice()
        .join(",");

    CgroupBuilder::new("test_launcher")
        .cpu()
        .cpus(cpus)
        .mems("0".to_owned())
        .done()
        .build(hier)
}

/// Calls strace on the input pid and stores the strace output to the specified path
async fn strace_child<P>(path: P, pid: nix::unistd::Pid)
where
    P: AsRef<Path>,
{
    log::debug!("Starting strace on child");
    let mut strace_handle = tokio::process::Command::new("strace")
        .args(&["-f", "-p", &pid.to_string()])
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to start strace");

    if let Some(output) = strace_handle.stderr.take() {
        let mut buf_stdout = tokio::io::BufReader::new(output);
        let mut strace_file = tokio::fs::File::create(&path)
            .await
            .expect("Failed to crete strace file");

        tokio::io::copy_buf(&mut buf_stdout, &mut strace_file)
            .await
            .expect("Could not copy strace output");
    }
}
