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
use cgroups_rs::Cgroup;
use std::time::Duration;

/// Collects the usage in bytes from the test cgroup
pub(crate) struct Memory {
    freq: Duration,
    #[allow(dead_code)]
    cgroup: Cgroup,
}

impl Memory {
    pub(crate) fn new(freq: Duration, cgroup: Cgroup) -> Memory {
        Memory { freq, cgroup }
    }
}

impl Metric for Memory {
    type Value = u64;

    fn frequency(&self) -> Duration {
        self.freq
    }

    fn collect(&self) -> u64 {
        // Get the memory usage from the cgroup only
        let mem_controller: &cgroups_rs::memory::MemController =
            self.cgroup.controller_of().unwrap();
        mem_controller.memory_stat().usage_in_bytes

        // Get the memory usage from the whole system
        // let meminfo =
        //     std::fs::read_to_string("/proc/meminfo").expect("Could not read proc meminfo");
        // for line in meminfo.lines() {
        //     if line.starts_with("MemFree:") {
        //         let kbytes = line
        //             .split_whitespace()
        //             .nth(1)
        //             .map(|s| s.parse::<u64>().unwrap_or_default())
        //             .unwrap_or_default();
        //         return kbytes * 1000;
        //     }
        // }
        // 0
    }
}
