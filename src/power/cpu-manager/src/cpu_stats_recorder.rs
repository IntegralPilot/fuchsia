// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use anyhow::{Result, format_err};
use async_trait::async_trait;
use fidl_fuchsia_kernel as fstats;
use fuchsia_async as fasync;
use fuchsia_inspect::{self as inspect};
use fuchsia_sync::Mutex;
use futures::future::{FutureExt as _, LocalBoxFuture};
use futures::stream::{FuturesUnordered, StreamExt};
use serde_derive::Deserialize;
use serde_json as json;
use state_recorder::{NumericStateRecorder, RecorderOptions, StateRecorderManager, units};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use zx;

/// Node: CpuStatsRecorder
///
/// Summary: Polls the kernel stats service to collect CPU load and energy estimates.
///          Data is recorded to Inspect using NumericStateRecorder.
///
/// Handles Messages: : N/A
///
/// Sends Messages: N/A
///
/// FIDL dependencies:
///     - fuchsia.kernel.Stats: the node connects to this service to query kernel information

pub struct CpuStatsRecorderBuilder {
    stats_svc_proxy: Option<fstats::StatsProxy>,
    state_recorder_manager: Option<Arc<Mutex<StateRecorderManager>>>,
    poll_interval: zx::MonotonicDuration,
    num_history_entries: usize,
    poll_interval_s: f64,
    inspector: Option<inspect::Inspector>,
}

impl CpuStatsRecorderBuilder {
    pub fn new_from_json(json_data: json::Value, _nodes: &HashMap<String, Rc<dyn Node>>) -> Self {
        #[derive(Deserialize)]
        struct Config {
            poll_interval_s: f64,
            num_history_entries: usize,
        }

        let config: Config = json::from_value(json_data["config"].clone())
            .expect("CpuStatsRecorder 'config' is required and must match schema");

        Self {
            stats_svc_proxy: None,
            state_recorder_manager: None,
            poll_interval: zx::MonotonicDuration::from_seconds(config.poll_interval_s as i64),
            num_history_entries: config.num_history_entries,
            poll_interval_s: config.poll_interval_s,
            inspector: None,
        }
    }

    #[cfg(test)]
    pub fn with_inspector(mut self, inspector: inspect::Inspector) -> Self {
        self.inspector = Some(inspector);
        self
    }

    #[cfg(test)]
    pub fn with_proxy(mut self, proxy: fstats::StatsProxy) -> Self {
        self.stats_svc_proxy = Some(proxy);
        self
    }

    #[cfg(test)]
    pub fn with_state_recorder_manager(
        mut self,
        manager: Arc<Mutex<StateRecorderManager>>,
    ) -> Self {
        self.state_recorder_manager = Some(manager);
        self
    }

    pub async fn build(
        self,
        futures_out: &FuturesUnordered<LocalBoxFuture<'_, ()>>,
    ) -> Result<Rc<CpuStatsRecorder>> {
        let stats_svc_proxy = if let Some(proxy) = self.stats_svc_proxy {
            proxy
        } else {
            fuchsia_component::client::connect_to_protocol::<fstats::StatsMarker>()?
        };

        let state_recorder_manager =
            self.state_recorder_manager.unwrap_or_else(|| state_recorder::manager());

        let inspector = self.inspector.unwrap_or_else(|| inspect::component::inspector().clone());
        let inspect_root = inspector.root().create_child("CpuStatsRecorder");
        inspect_root.record_double("poll_interval_s", self.poll_interval_s);
        inspect_root.record_uint("num_history_entries", self.num_history_entries as u64);

        let node = Rc::new(CpuStatsRecorder {
            stats_svc_proxy,
            state: RefCell::new(None),
            last_sample: RefCell::new(None),
            state_recorder_manager,
            poll_interval: self.poll_interval,
            num_history_entries: self.num_history_entries,
            _inspect_root: inspect_root,
        });

        futures_out.push(node.clone().poll_loop());

        Ok(node)
    }
}

pub struct CpuStatsRecorder {
    stats_svc_proxy: fstats::StatsProxy,
    state: RefCell<Option<CpuRecorders>>,
    last_sample: RefCell<Option<LastSample>>,
    state_recorder_manager: Arc<Mutex<StateRecorderManager>>,
    poll_interval: zx::MonotonicDuration,
    num_history_entries: usize,
    _inspect_root: inspect::Node,
}

struct CpuRecorders {
    cpu_load: CpuStatsHistory,
    power: CpuStatsHistory,
}

struct CpuStatsHistory {
    recorder: NumericStateRecorder<f32>,
}

impl CpuStatsHistory {
    fn new(
        name: String,
        manager: Arc<Mutex<StateRecorderManager>>,
        units: state_recorder::Units,
        capacity: usize,
    ) -> Result<Self> {
        let recorder = NumericStateRecorder::new(
            name,
            c"cpu_manager",
            units,
            None,
            RecorderOptions {
                lazy_record: true,
                capacity,
                manager: Some(manager),
                ..Default::default()
            },
        )
        .map_err(|e| format_err!("Failed to create recorder: {:?}", e))?;

        Ok(Self { recorder })
    }

    fn record(&mut self, value: f32) {
        self.recorder.record(value);
    }
}

struct LastSample {
    busy_time: u64,
    energy: u64,
    boot_timestamp_ns: i64,
}

impl CpuStatsRecorder {
    #[cfg(not(test))]
    fn get_now_ns(&self) -> i64 {
        zx::BootInstant::get().into_nanos()
    }

    #[cfg(test)]
    fn get_now_ns(&self) -> i64 {
        fasync::MonotonicInstant::now().into_nanos()
    }

    fn poll_loop<'a>(self: Rc<Self>) -> LocalBoxFuture<'a, ()> {
        async move {
            let mut interval = fasync::Interval::new(self.poll_interval);

            loop {
                if let Err(e) = self.sample().await {
                    log::error!("CpuStatsRecorder sample failed: {:?}", e);
                }
                interval.next().await;
            }
        }
        .boxed_local()
    }

    async fn sample(&self) -> Result<()> {
        let cpu_stats = self
            .stats_svc_proxy
            .get_cpu_stats()
            .await
            .map_err(|e| format_err!("get_cpu_stats failed: {}", e))?;

        let per_cpu_stats =
            cpu_stats.per_cpu_stats.ok_or_else(|| format_err!("No per_cpu_stats"))?;
        let num_cpus = cpu_stats.actual_num_cpus as f64;

        let timestamp = self.get_now_ns();
        let mut total_busy_time = 0;
        let mut total_energy = 0;

        for stats in per_cpu_stats {
            if let Some(val) = stats.normalized_busy_time {
                total_busy_time += val as u64;
            }

            if let Some(val) = stats.active_energy_consumption_nj {
                total_energy += val;
            }

            if let Some(val) = stats.idle_energy_consumption_nj {
                total_energy += val;
            }
        }

        let mut last_sample = self.last_sample.borrow_mut();
        if let Some(last) = last_sample.as_ref() {
            let mut state = self.state.borrow_mut();
            let recorders = if let Some(recorders) = state.as_mut() {
                recorders
            } else {
                let cpu_load = CpuStatsHistory::new(
                    "cpu_load".to_string(),
                    self.state_recorder_manager.clone(),
                    units!(Percent),
                    self.num_history_entries,
                )?;

                let power = CpuStatsHistory::new(
                    "power".to_string(),
                    self.state_recorder_manager.clone(),
                    units!(Milli, Watts),
                    self.num_history_entries,
                )?;

                state.insert(CpuRecorders { cpu_load, power })
            };

            let delta_time_ns = timestamp - last.boot_timestamp_ns;
            if delta_time_ns > 0 {
                let delta_time = delta_time_ns as f64;
                let delta_busy = total_busy_time.wrapping_sub(last.busy_time);
                let delta_energy = total_energy.wrapping_sub(last.energy);

                // Calculate CPU load such that the maximum load is 100%.
                let cpu_load_percent = (delta_busy as f64 / delta_time / num_cpus) * 100.0;
                let power_mw = (delta_energy as f64 / delta_time) * 1000.0;

                recorders.cpu_load.record(cpu_load_percent as f32);
                recorders.power.record(power_mw as f32);
            }
        }

        *last_sample = Some(LastSample {
            busy_time: total_busy_time,
            energy: total_energy,
            boot_timestamp_ns: timestamp,
        });

        Ok(())
    }
}

#[async_trait(?Send)]
impl Node for CpuStatsRecorder {
    fn name(&self) -> String {
        "CpuStatsRecorder".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_utils::PollExt as _;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fuchsia_inspect as inspect;

    fn setup_fake_service<F>(mut get_stats: F) -> fstats::StatsProxy
    where
        F: FnMut() -> fstats::CpuStats + 'static,
    {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<fstats::StatsMarker>();
        fasync::Task::local(async move {
            use futures::TryStreamExt;
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    fstats::StatsRequest::GetCpuStats { responder } => {
                        responder.send(&get_stats()).unwrap();
                    }
                    _ => {}
                }
            }
        })
        .detach();
        proxy
    }

    #[fuchsia::test]
    async fn test_init_from_config() {
        let futures_out = FuturesUnordered::new();
        let config = json::json!({
            "type": "CpuStatsRecorder",
            "name": "cpu_stats_recorder",
            "config": {
                "poll_interval_s": 0.5,
                "num_history_entries": 20
            }
        });

        let inspector = inspect::Inspector::default();
        let builder = CpuStatsRecorderBuilder::new_from_json(config, &HashMap::new())
            .with_inspector(inspector.clone());

        let _node = builder.build(&futures_out).await.unwrap();

        assert_data_tree!(
            inspector,
            root: {
                CpuStatsRecorder: {
                    poll_interval_s: 0.5,
                    num_history_entries: 20u64,
                }
            }
        );
    }

    #[fuchsia::test]
    fn test_recorder_sampling() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let inspector = inspect::Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let stats_sequence = Rc::new(RefCell::new(vec![
            fstats::CpuStats {
                actual_num_cpus: 2,
                per_cpu_stats: Some(vec![
                    fstats::PerCpuStats {
                        normalized_busy_time: Some(0),
                        active_energy_consumption_nj: Some(0),
                        idle_energy_consumption_nj: Some(0),
                        ..Default::default()
                    },
                    fstats::PerCpuStats {
                        normalized_busy_time: Some(0),
                        active_energy_consumption_nj: Some(0),
                        idle_energy_consumption_nj: Some(0),
                        ..Default::default()
                    },
                ]),
            },
            fstats::CpuStats {
                actual_num_cpus: 2,
                per_cpu_stats: Some(vec![
                    fstats::PerCpuStats {
                        normalized_busy_time: Some(40_000_000),
                        active_energy_consumption_nj: Some(100_000_000),
                        idle_energy_consumption_nj: Some(50_000_000),
                        ..Default::default()
                    },
                    fstats::PerCpuStats {
                        normalized_busy_time: Some(10_000_000),
                        active_energy_consumption_nj: Some(200_000_000),
                        idle_energy_consumption_nj: Some(75_000_000),
                        ..Default::default()
                    },
                ]),
            },
            fstats::CpuStats {
                actual_num_cpus: 2,
                per_cpu_stats: Some(vec![
                    fstats::PerCpuStats {
                        normalized_busy_time: Some(60_000_000),
                        active_energy_consumption_nj: Some(150_000_000),
                        idle_energy_consumption_nj: Some(50_000_000),
                        ..Default::default()
                    },
                    fstats::PerCpuStats {
                        normalized_busy_time: Some(15_000_000),
                        active_energy_consumption_nj: Some(100_000_000),
                        idle_energy_consumption_nj: Some(300_000_000),
                        ..Default::default()
                    },
                ]),
            },
        ]));

        let stats_sequence_clone = stats_sequence.clone();
        let stats_proxy = setup_fake_service(move || stats_sequence_clone.borrow_mut().remove(0));
        let futures_out = FuturesUnordered::new();

        let build_fut = CpuStatsRecorderBuilder::new_from_json(
            json::json!({
                "type": "CpuStatsRecorder",
                "name": "cpu_stats_recorder",
                "config": {
                    "poll_interval_s": 1.0,
                    "num_history_entries": 10
                }
            }),
            &HashMap::new(),
        )
        .with_proxy(stats_proxy)
        .with_state_recorder_manager(manager)
        .with_inspector(inspector.clone())
        .build(&futures_out);
        futures::pin_mut!(build_fut);

        let node = executor.run_until_stalled(&mut build_fut).unwrap().unwrap();

        // Initial sample
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let sample_fut = node.sample();
        futures::pin_mut!(sample_fut);
        executor.run_until_stalled(&mut sample_fut).unwrap().unwrap();
        let _ = executor.run_until_stalled(&mut async {}.boxed_local());

        // Second sample
        // Advance time by 1s
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(1_000_000_000));
        let _ = executor.run_until_stalled(&mut async {}.boxed_local());
        let sample_fut = node.sample();
        futures::pin_mut!(sample_fut);
        executor.run_until_stalled(&mut sample_fut).unwrap().unwrap();
        let _ = executor.run_until_stalled(&mut async {}.boxed_local());

        // Third sample
        // Advance time by another 1s
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(2_000_000_000));
        let _ = executor.run_until_stalled(&mut async {}.boxed_local());
        let sample_fut = node.sample();
        futures::pin_mut!(sample_fut);
        executor.run_until_stalled(&mut sample_fut).unwrap().unwrap();
        let _ = executor.run_until_stalled(&mut async {}.boxed_local());

        // Verify Inspect data
        executor
            .run_until_stalled(
                &mut async {
                    assert_data_tree!(
                        inspector,
                        root: {
                            power_observability_state_recorders: {
                                cpu_load: contains {
                                    history: contains {
                                        shards: contains {
                                            "0": {
                                                times: AnyProperty,
                                                values: vec![2.5f64, 1.25f64],
                                            }
                                        }
                                    }
                                },
                                power: contains {
                                    history: contains {
                                        shards: contains {
                                            "0": {
                                                times: AnyProperty,
                                                values: vec![425.0f64, 175.0f64],
                                            }
                                        }
                                    }
                                }
                            },
                            CpuStatsRecorder: {
                                poll_interval_s: 1.0,
                                num_history_entries: 10u64,
                            }
                        }
                    );
                }
                .boxed_local(),
            )
            .unwrap();
    }
}
