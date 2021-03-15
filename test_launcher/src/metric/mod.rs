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

use chrono::{serde::ts_milliseconds, Utc};
use color_eyre::eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use tokio::sync::oneshot;

pub(crate) mod fd_count;
pub(crate) mod memory;

pub(crate) use fd_count::FdCount;
pub(crate) use memory::Memory;

pub(crate) trait Metric {
    /// The value type collected by the Metric
    type Value;

    /// The frequency on which this measurement is collected
    fn frequency(&self) -> Duration;

    /// Gets called to collect the metric value
    fn collect(&self) -> Self::Value;
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MetricRecord<T> {
    #[serde(with = "ts_milliseconds")]
    pub(crate) timestamp: chrono::DateTime<Utc>,
    pub(crate) value: T,
}

impl<T> MetricRecord<T> {
    fn new(value: T) -> MetricRecord<T> {
        MetricRecord {
            timestamp: Utc::now(),
            value,
        }
    }
}

/// A trait to abstract the destination storage and format for the metrics
pub(crate) trait CollectionBackend<M: Metric> {
    fn record_chunk(&mut self, chunk: &[MetricRecord<<M as Metric>::Value>]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
}

/// Collect the metrics to a .csv file
pub(crate) struct CSVBackend<W: std::io::Write> {
    csv: csv::Writer<W>,
}

impl<W> CSVBackend<W>
where
    W: std::io::Write,
{
    /// Create a CSVBackend out of a std::io::Write
    pub(crate) fn from_writer(w: W) -> Self {
        Self {
            csv: csv::Writer::from_writer(w),
        }
    }
}

impl<W, M> CollectionBackend<M> for CSVBackend<W>
where
    W: std::io::Write,
    M: Metric,
    <M as Metric>::Value: serde::Serialize,
{
    fn record_chunk(&mut self, chunk: &[MetricRecord<<M as Metric>::Value>]) -> Result<()> {
        for record in chunk.iter() {
            self.csv.serialize(record)?;
        }
        <Self as CollectionBackend<M>>::flush(self)
    }

    fn flush(&mut self) -> Result<()> {
        self.csv.flush().wrap_err("Failed to flush output csv")
    }
}

/// Takes a metric and a collection backend
pub(crate) struct MetricCollector {
    stop: oneshot::Sender<u8>,
    handle: tokio::task::JoinHandle<()>,
}

impl MetricCollector {
    pub(crate) async fn stop(self) -> Result<()> {
        self.stop.send(1).unwrap();
        self.handle
            .await
            .wrap_err("Failed to wait for collector termination")
    }
}

/// Spawns a task that collects the metric values into a csv file in the provided path
pub(crate) fn collect_to_csv<M, P>(
    rt: &tokio::runtime::Runtime,
    metric: M,
    path: P,
) -> Result<MetricCollector>
where
    M: Metric + Send + 'static,
    <M as Metric>::Value: Send + Serialize,
    P: AsRef<Path>,
{
    let file = std::fs::File::create(&path).wrap_err("Failed to create output CSV file")?;
    let csv = CSVBackend::from_writer(file);
    let (stop, wait_stop) = oneshot::channel();
    let handle = rt.spawn(collection_task(metric, csv, wait_stop));
    Ok(MetricCollector { stop, handle })
}

/// Collects metric values and stores them by chuncks to a collection backend
async fn collection_task<M, B>(metric: M, mut backend: B, mut wait_stop: oneshot::Receiver<u8>)
where
    M: Metric,
    B: CollectionBackend<M> + 'static,
{
    let mut interval = tokio::time::interval(metric.frequency());

    // TODO find some appropiate size for the chunks
    let mut chunk = Vec::with_capacity(10000);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let value = tokio::task::block_in_place(|| metric.collect());
                chunk.push(MetricRecord::new(value));

                // TODO do this asynchronously
                if chunk.len() == chunk.capacity() {
                    backend.record_chunk(&chunk).unwrap();
                    chunk.clear();
                }
            },
            _ = &mut wait_stop => break,
        }
    }

    backend.record_chunk(&chunk).unwrap();
}
