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

use npk::manifest::Version;
use parse_duration::parse;
use serde::{
    de::{self, Deserializer},
    Deserialize, Serialize,
};
use std::{path::PathBuf, time::Duration};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
pub struct LauncherOpt {
    #[structopt(
        short,
        long,
        help = "Path to the configuration file",
        default_value = "launcher.yaml"
    )]
    pub config: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeDuration(#[serde(deserialize_with = "deserialize_duration")] Duration);

impl From<TimeDuration> for Duration {
    fn from(duration: TimeDuration) -> Self {
        duration.0
    }
}

/// Measures the system memory
#[derive(Debug, Serialize, Deserialize)]
pub struct SystemMemory {
    pub freq: TimeDuration,
}

/// Measures the opened files
#[derive(Debug, Serialize, Deserialize)]
pub struct FileDescriptors {
    pub freq: TimeDuration,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Metrics {
    /// Measures the consumed memory
    pub memory: SystemMemory,
    /// Counts the open files
    pub file_descriptors: FileDescriptors,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Container {
    /// Name for the container
    pub name: String,
    /// Version of the container
    pub version: Version,
    /// Set some interval to start the container
    pub start_after: TimeDuration,
    /// Runtime time
    pub stop_after: TimeDuration,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResultsOutput {
    /// Target directory where to create the result entry
    pub out_dir: PathBuf,
    /// The name of the directory where all the results are written to
    pub entry_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProgramOpt {
    /// Test duration
    pub time: TimeDuration,
    /// CPU IDs used by the spawn process
    pub cpus: Vec<u8>,
    /// Metrics configuration
    pub metrics: Metrics,
    /// Container orchestration
    pub containers: Vec<Container>,
    /// Output directory where to create the result entry
    pub results: ResultsOutput,
    /// Context size for log errors captures
    #[serde(default = "default_error_context_size")]
    pub error_context_lines: usize,
    /// Collect strace output from the runtime
    #[serde(default = "default_strace_setting")]
    pub strace: bool,
}

fn default_error_context_size() -> usize {
    3
}

fn default_strace_setting() -> bool {
    false
}

fn deserialize_duration<'de, D>(de: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Duration;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(formatter, "a string with a duration")
        }
        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse(s).map_err(|_| de::Error::invalid_value(de::Unexpected::Str(s), &self))
        }
    }
    de.deserialize_string(V {})
}
