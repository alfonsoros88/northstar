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

use super::Metric;
use std::time::Duration;

/// Collects the number of files open by the "tester" process
pub(crate) struct FdCount {
    freq: Duration,
    pid: i32,
}

impl FdCount {
    pub(crate) fn new(freq: Duration, pid: i32) -> FdCount {
        FdCount { freq, pid }
    }
}

impl Metric for FdCount {
    type Value = u64;

    fn frequency(&self) -> Duration {
        self.freq
    }

    fn collect(&self) -> u64 {
        match std::fs::read_dir(format!("/proc/{}/fd", self.pid)) {
            Ok(entries) => entries.count() as u64,
            Err(_) => 0,
        }
    }
}
