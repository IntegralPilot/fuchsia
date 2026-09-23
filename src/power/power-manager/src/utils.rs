// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_temperature as ftemperature;
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_component::client as fclient;
use futures::future::FutureExt;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryFutureExt, TryStreamExt};
use zx::MonotonicDuration;

/// Logs an error message if the provided `cond` evaluates to false. Also passes the same expression
/// and message into `debug_assert!`, which will panic if debug assertions are enabled.
#[macro_export]
macro_rules! log_if_false_and_debug_assert {
    ($cond:expr, $msg:expr) => {
        if !($cond) {
            log::error!($msg);
            debug_assert!($cond, $msg);
        }
    };
    ($cond:expr, $fmt:expr, $($arg:tt)+) => {
        if !($cond) {
            log::error!($fmt, $($arg)+);
            debug_assert!($cond, $fmt, $($arg)+);
        }
    };
}

#[cfg(test)]
mod log_err_with_debug_assert_tests {

    /// Tests that `log_if_false_and_debug_assert` panics for a false expression when debug
    /// assertions are enabled.
    #[test] // Use instead of #[fuchsia::test] to suppress ERROR logs that cause test failure
    #[should_panic(expected = "this will panic")]
    #[cfg(debug_assertions)]
    fn test_debug_assert() {
        log_if_false_and_debug_assert!(true, "this will not panic");
        log_if_false_and_debug_assert!(false, "this will panic");
    }

    /// Tests that `log_if_false_and_debug_assert` does not panic for a false expression when debug
    /// assertions are not enabled.
    #[test] // Use instead of #[fuchsia::test] to suppress ERROR logs that cause test failure
    #[cfg(not(debug_assertions))]
    fn test_non_debug_assert() {
        log_if_false_and_debug_assert!(true, "this will not panic");
        log_if_false_and_debug_assert!(false, "this will not panic either");
    }
}

use fidl_fuchsia_metrics::HistogramBucket;

/// Convenient wrapper for creating and storing an integer histogram to use with Cobalt.
pub struct CobaltIntHistogram {
    /// Underlying histogram data storage.
    data: Vec<HistogramBucket>,

    /// Number of data values that have been added to the histogram.
    data_count: u32,

    /// Histogram configuration parameters.
    config: CobaltIntHistogramConfig,
}

/// Histogram configuration parameters used by CobaltIntHistogram.
pub struct CobaltIntHistogramConfig {
    pub floor: i64,
    pub num_buckets: u32,
    pub step_size: u32,
}

impl CobaltIntHistogram {
    /// Create a new CobaltIntHistogram.
    pub fn new(config: CobaltIntHistogramConfig) -> Self {
        Self { data: Self::new_vec(config.num_buckets), data_count: 0, config }
    }

    /// Create a new Vec<HistogramBucket> that represents the underlying histogram storage. Two
    /// extra buckets are added for underflow and overflow.
    fn new_vec(num_buckets: u32) -> Vec<HistogramBucket> {
        (0..num_buckets + 2).map(|i| HistogramBucket { index: i, count: 0 }).collect()
    }

    /// Add a data value to the histogram.
    pub fn add_data(&mut self, n: i64) {
        // Add one to index to account for underflow bucket at index 0
        let mut index = 1 + (n - self.config.floor) / self.config.step_size as i64;

        // Clamp index to 0 and self.data.len() - 1, which Cobalt uses for underflow and overflow,
        // respectively
        index = num_traits::clamp(index, 0, self.data.len() as i64 - 1);

        self.data[index as usize].count += 1;
        self.data_count += 1;
    }

    /// Get the number of data elements that have been added to the histogram.
    pub fn count(&self) -> u32 {
        self.data_count
    }

    /// Clear the histogram.
    pub fn clear(&mut self) {
        self.data = Self::new_vec(self.config.num_buckets);
        self.data_count = 0;
    }

    /// Get the underlying Vec<HistogramBucket> of the histogram.
    pub fn get_data(&self) -> Vec<HistogramBucket> {
        self.data.clone()
    }
}

const TEMPERATURE_DRIVER_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(60);

pub async fn get_temperature_driver_proxy(
    sensor_name: &str,
) -> Result<ftemperature::DeviceProxy, Error> {
    const TEMPERATURE_DRIVER_DIRS: [&str; 2] = ["/dev/class/temperature", "/dev/class/thermal"];

    // Search for the temperature driver across DevFS directories and FIDL service instances
    // concurrently. The first watcher to find the matching sensor will return, cancelling
    // remaining watchers.
    let mut futures = FuturesUnordered::new();
    for dir_path in TEMPERATURE_DRIVER_DIRS {
        futures.push(get_temperature_driver_proxy_from_dir(dir_path, sensor_name).boxed());
    }
    futures.push(get_temperature_driver_proxy_from_svc(sensor_name).boxed());

    let mut debug_messages = Vec::new();
    while let Some(result) = futures.next().await {
        match result {
            Ok(proxy) => return Ok(proxy),
            // It is ok that a driver directory or svc doesn't have the sensor or times out waiting
            // for `sensor_name` to show up. Cache the error.
            Err(err) => debug_messages.push(err),
        };
    }
    // If `sensor_name` is not found across all sources, print all the error messages to help with
    // debugging.
    for (i, message) in debug_messages.into_iter().enumerate() {
        log::error!("Failed to find temperature driver debug message [{}]: {:?}", i, message);
    }
    return Err(anyhow::anyhow!(
        "Failed to find temperature driver with sensor name: {:?}.",
        sensor_name
    ));
}

async fn get_temperature_driver_proxy_from_dir(
    dir_path: &str,
    required_sensor_name: &str,
) -> Result<ftemperature::DeviceProxy, Error> {
    let dir = fuchsia_fs::directory::open_in_namespace(dir_path, fuchsia_fs::PERM_READABLE)?;

    let mut watcher = fuchsia_fs::directory::Watcher::new(&dir).await.map_err(|e| {
        anyhow::anyhow!("Failed to create watcher for directory {:?}: {:?}", dir_path, e)
    })?;

    loop {
        let next = watcher
            .try_next()
            .map_err(|e| anyhow::anyhow!("Failed to get next watch message: {e:?}"))
            .on_timeout(TEMPERATURE_DRIVER_TIMEOUT.after_now(), || {
                Err(anyhow::anyhow!("Timeout waiting for next watcher message."))
            })
            .await?;

        if let Some(watch_msg) = next {
            let filename = watch_msg
                .filename
                .as_path()
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Failed to convert filename to string"))?;
            if filename != "." {
                if watch_msg.event == fuchsia_fs::directory::WatchEvent::ADD_FILE
                    || watch_msg.event == fuchsia_fs::directory::WatchEvent::EXISTING
                {
                    let proxy = fuchsia_component::client::connect_to_named_protocol_at_dir_root::<
                        ftemperature::DeviceMarker,
                    >(&dir, &filename)?;
                    let sensor_name = proxy
                        .get_sensor_name()
                        .await
                        .map_err(|e| anyhow::anyhow!("GetSensorName failed with err: {:?}", e))?;
                    log::debug!(
                        filename, dir_path, sensor_name = sensor_name.as_str();
                        "Temperature driver detected"
                    );

                    if required_sensor_name == sensor_name {
                        return Ok(proxy);
                    }
                }
            }
        } else {
            return Err(anyhow::anyhow!("Directory watcher returned None entry."));
        }
    }
}

async fn get_temperature_driver_proxy_from_svc(
    required_sensor_name: &str,
) -> Result<ftemperature::DeviceProxy, Error> {
    let mut watcher = fclient::Service::open(ftemperature::ServiceMarker)?.watch().await?;

    loop {
        let next = watcher
            .try_next()
            .map_err(|e| anyhow::anyhow!("Failed to get next watch message: {e:?}"))
            .on_timeout(TEMPERATURE_DRIVER_TIMEOUT.after_now(), || {
                Err(anyhow::anyhow!("Timeout waiting for next watcher message."))
            })
            .await?;

        if let Some(instance) = next {
            let proxy: ftemperature::DeviceProxy = instance.connect_to_device()?;
            let sensor_name = proxy
                .get_sensor_name()
                .await
                .map_err(|e| anyhow::anyhow!("GetSensorName failed with err: {:?}", e))?;
            log::debug!(
                "Temperature driver detected: {} {}",
                instance.instance_name(),
                sensor_name
            );

            if required_sensor_name == sensor_name {
                return Ok(proxy);
            }
        } else {
            return Err(anyhow::anyhow!("Directory watcher returned None entry."));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CobaltIntHistogram: tests that data added to the CobaltIntHistogram is correctly counted and
    /// bucketed.
    #[fuchsia::test]
    fn test_cobalt_histogram_data() {
        // Create the histogram and verify initial data count is 0
        let mut hist = CobaltIntHistogram::new(CobaltIntHistogramConfig {
            floor: 50,
            step_size: 10,
            num_buckets: 3,
        });
        assert_eq!(hist.count(), 0);

        // Add some arbitrary values, making sure some do not land on the bucket boundary to further
        // verify the bucketing logic
        hist.add_data(50);
        hist.add_data(65);
        hist.add_data(75);
        hist.add_data(79);

        // Verify the values were counted and bucketed properly
        assert_eq!(hist.count(), 4);
        assert_eq!(
            hist.get_data(),
            vec![
                HistogramBucket { index: 0, count: 0 }, // underflow
                HistogramBucket { index: 1, count: 1 },
                HistogramBucket { index: 2, count: 1 },
                HistogramBucket { index: 3, count: 2 },
                HistogramBucket { index: 4, count: 0 } // overflow
            ]
        );

        // Verify `clear` works as expected
        hist.clear();
        assert_eq!(hist.count(), 0);
        assert_eq!(
            hist.get_data(),
            vec![
                HistogramBucket { index: 0, count: 0 }, // underflow
                HistogramBucket { index: 1, count: 0 },
                HistogramBucket { index: 2, count: 0 },
                HistogramBucket { index: 3, count: 0 },
                HistogramBucket { index: 4, count: 0 }, // overflow
            ]
        );
    }

    /// CobaltIntHistogram: tests that invalid data values are logged in the correct
    /// underflow/overflow buckets.
    #[fuchsia::test]
    fn test_cobalt_histogram_invalid_data() {
        let mut hist = CobaltIntHistogram::new(CobaltIntHistogramConfig {
            floor: 0,
            step_size: 1,
            num_buckets: 2,
        });

        hist.add_data(-2);
        hist.add_data(-1);
        hist.add_data(0);
        hist.add_data(1);
        hist.add_data(2);

        assert_eq!(
            hist.get_data(),
            vec![
                HistogramBucket { index: 0, count: 2 }, // underflow
                HistogramBucket { index: 1, count: 1 },
                HistogramBucket { index: 2, count: 1 },
                HistogramBucket { index: 3, count: 1 } // overflow
            ]
        );
    }
}

#[cfg(test)]
pub mod run_all_tasks_until_stalled {
    /// Runs all tasks known to the current executor until stalled.
    ///
    /// This is a useful convenience function to run `fuchsia_async::Task`s where directly polling
    /// them is either inconvenient or not possible (e.g., detached).
    pub fn run_all_tasks_until_stalled(executor: &mut fuchsia_async::TestExecutor) {
        assert!(executor.run_until_stalled(&mut futures::future::pending::<()>()).is_pending());
    }

    mod tests {
        use super::run_all_tasks_until_stalled;
        use std::cell::Cell;
        use std::rc::Rc;

        #[fuchsia::test]
        fn test_run_all_tasks_until_stalled() {
            let mut executor = fuchsia_async::TestExecutor::new_with_fake_time();

            let completed = Rc::new(Cell::new(false));
            let completed_clone = completed.clone();

            let _task = fuchsia_async::Task::local(async move {
                completed_clone.set(true);
            });

            assert_eq!(completed.get(), false);
            run_all_tasks_until_stalled(&mut executor);
            assert_eq!(completed.get(), true);
        }
    }
}
