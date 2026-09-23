// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_starnix_psi::{
    PsiProviderGetMemoryPressureStatsResponse, PsiProviderRequest, PsiProviderRequestStream,
    PsiProviderWatchMemoryStallRequest, PsiProviderWatchMemoryStallResponse, PsiStats,
};
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::warn;
use std::sync::{Arc, Weak};
use {fidl_fuchsia_kernel as fkernel, fuchsia_async as fasync};

mod history;
use history::{History, SAMPLING_RATE};

trait DataSource: Send + Sync {
    /// Captures the current timestamp and the memory stall stats.
    fn capture(&self) -> (zx::MonotonicInstant, zx::MemoryStall);

    fn watch(
        &self,
        kind: zx::MemoryStallKind,
        threshold: zx::MonotonicDuration,
        window: zx::MonotonicDuration,
    ) -> Result<zx::Event, zx::Status>;
}

struct RealDataSource {
    stall_resource: zx::Resource,
}

impl DataSource for RealDataSource {
    fn capture(&self) -> (zx::MonotonicInstant, zx::MemoryStall) {
        let now = zx::MonotonicInstant::get();
        let stat = self.stall_resource.memory_stall().unwrap();
        (now, stat)
    }

    fn watch(
        &self,
        kind: zx::MemoryStallKind,
        threshold: zx::MonotonicDuration,
        window: zx::MonotonicDuration,
    ) -> Result<zx::Event, zx::Status> {
        self.stall_resource.watch_memory_stall(kind, threshold, window)
    }
}

struct PsiProvider {
    data_source: Box<dyn DataSource>,
    history: Mutex<History>,
    history_updater_task: Option<fasync::Task<()>>,
}

impl PsiProvider {
    pub fn new(stall_resource: zx::Resource) -> Arc<PsiProvider> {
        // Initialize history with one sample only (the current data point).
        let data_source = RealDataSource { stall_resource };
        let (now, stat) = data_source.capture();
        let history = History::new(
            zx::MonotonicDuration::from_nanos(stat.stall_time_some),
            zx::MonotonicDuration::from_nanos(stat.stall_time_full),
            now,
        );

        // Instantiate the PsiProvider along with the task that will periodically update its history
        // in the background.
        Arc::new_cyclic(|weak_ptr| {
            let history_updater_task = fasync::Task::spawn(history_updater(weak_ptr.clone()));
            PsiProvider {
                data_source: Box::new(data_source),
                history: Mutex::new(history),
                history_updater_task: Some(history_updater_task),
            }
        })
    }

    /// Unlike the regular `new` constructor, this one does not start the background task and lets
    /// one inject a fake `DataSource`.
    #[cfg(test)]
    fn new_for_test(data_source: impl DataSource + 'static) -> Arc<PsiProvider> {
        let (now, stat) = data_source.capture();
        let history = History::new(
            zx::MonotonicDuration::from_nanos(stat.stall_time_some),
            zx::MonotonicDuration::from_nanos(stat.stall_time_full),
            now,
        );

        Arc::new(PsiProvider {
            data_source: Box::new(data_source),
            history: Mutex::new(history),
            history_updater_task: None,
        })
    }

    /// Responds to client requests.
    pub async fn serve(&self, mut stream: PsiProviderRequestStream) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await? {
            match event {
                PsiProviderRequest::GetMemoryPressureStats { responder } => {
                    let response = self.query();
                    if let Err(e) = responder.send(Ok(&response)) {
                        warn!("error responding to stats request: {e}");
                    }
                }
                PsiProviderRequest::WatchMemoryStall { payload, responder } => {
                    let response = match payload {
                        PsiProviderWatchMemoryStallRequest {
                            kind: Some(kind),
                            threshold: Some(threshold),
                            window: Some(window),
                            ..
                        } => {
                            if let Some(kind) = zx::MemoryStallKind::from_bits(kind) {
                                self.data_source
                                    .watch(
                                        kind,
                                        zx::MonotonicDuration::from_nanos(threshold),
                                        zx::MonotonicDuration::from_nanos(window),
                                    )
                                    .map(|event| PsiProviderWatchMemoryStallResponse {
                                        event: Some(event),
                                        ..Default::default()
                                    })
                            } else {
                                Err(zx::Status::INVALID_ARGS)
                            }
                        }
                        _ => Err(zx::Status::INVALID_ARGS),
                    };
                    let _ = responder.send(response.map_err(|e| e.into_raw()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Computes the current stats.
    fn query(&self) -> PsiProviderGetMemoryPressureStatsResponse {
        const DURATION_10: zx::MonotonicDuration = zx::Duration::from_seconds(10);
        const DURATION_60: zx::MonotonicDuration = zx::Duration::from_seconds(60);
        const DURATION_300: zx::MonotonicDuration = zx::Duration::from_seconds(300);
        const FACTOR_10: f64 = 1.0 / (DURATION_10.into_nanos() as f64);
        const FACTOR_60: f64 = 1.0 / (DURATION_60.into_nanos() as f64);
        const FACTOR_300: f64 = 1.0 / (DURATION_300.into_nanos() as f64);

        let (now, stat) = self.data_source.capture();

        // Retrieve the values 10, 60 and 300 seconds ago from the history.
        let guard = self.history.lock();
        let (some10, full10) = guard.query_at(now - DURATION_10);
        let (some60, full60) = guard.query_at(now - DURATION_60);
        let (some300, full300) = guard.query_at(now - DURATION_300);
        std::mem::drop(guard);

        let some = PsiStats {
            avg10: Some(
                ((stat.stall_time_some - some10.into_nanos()) as f64 * FACTOR_10).clamp(0.0, 1.0),
            ),
            avg60: Some(
                ((stat.stall_time_some - some60.into_nanos()) as f64 * FACTOR_60).clamp(0.0, 1.0),
            ),
            avg300: Some(
                ((stat.stall_time_some - some300.into_nanos()) as f64 * FACTOR_300).clamp(0.0, 1.0),
            ),
            total: Some(stat.stall_time_some),
            ..Default::default()
        };
        let full = PsiStats {
            avg10: Some(
                ((stat.stall_time_full - full10.into_nanos()) as f64 * FACTOR_10).clamp(0.0, 1.0),
            ),
            avg60: Some(
                ((stat.stall_time_full - full60.into_nanos()) as f64 * FACTOR_60).clamp(0.0, 1.0),
            ),
            avg300: Some(
                ((stat.stall_time_full - full300.into_nanos()) as f64 * FACTOR_300).clamp(0.0, 1.0),
            ),
            total: Some(stat.stall_time_full),
            ..Default::default()
        };

        PsiProviderGetMemoryPressureStatsResponse {
            some: Some(some),
            full: Some(full),
            ..Default::default()
        }
    }

    /// Adds a new sample containing the current stall counters to the history.
    fn update_history(&self) {
        let (now, stat) = self.data_source.capture();
        self.history.lock().add_new_sample(
            zx::MonotonicDuration::from_nanos(stat.stall_time_some),
            zx::MonotonicDuration::from_nanos(stat.stall_time_full),
            now,
        );
    }
}

impl Drop for PsiProvider {
    fn drop(&mut self) {
        if let Some(task) = self.history_updater_task.take() {
            let _ = task.abort();
        }
    }
}

async fn history_updater(target: Weak<PsiProvider>) {
    let mut deadline = zx::MonotonicInstant::after(SAMPLING_RATE);
    loop {
        fasync::Timer::new(deadline).await;
        deadline += SAMPLING_RATE;

        if let Some(target) = target.upgrade() {
            target.update_history();
        }
    }
}

fn get_stall_resource() -> Result<zx::Resource, Error> {
    let proxy = connect_to_protocol_sync::<fkernel::StallResourceMarker>()?;
    let resource = proxy.get(zx::MonotonicInstant::INFINITE)?;
    Ok(resource)
}

enum Services {
    PsiProvider(PsiProviderRequestStream),
}

#[fuchsia::main(logging_tags = ["starnix_psi_provider"])]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();

    let psi_provider = match get_stall_resource() {
        Ok(stall_resource) => {
            fs.dir("svc").add_fidl_service(Services::PsiProvider);
            Some(PsiProvider::new(stall_resource))
        }
        Err(_) => {
            warn!("failed to get the optional stall resource, PSI will not be available");
            None
        }
    };

    fs.take_and_serve_directory_handle()?;

    fs.for_each_concurrent(None, |request: Services| async {
        match request {
            Services::PsiProvider(stream) => psi_provider
                .clone()
                .expect("this service is only offered if the stall resource was found")
                .serve(stream)
                .await
                .expect("failed to serve starnix psi provider"),
        }
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeDataSource {
        result: Mutex<(zx::MonotonicInstant, zx::MemoryStall)>,
    }

    impl FakeDataSource {
        fn increment(&self, timestamp: zx::MonotonicDuration, some_factor: f64, full_factor: f64) {
            let mut guard = self.result.lock();
            guard.0 += timestamp;
            guard.1.stall_time_some += (timestamp.into_nanos() as f64 * some_factor) as i64;
            guard.1.stall_time_full += (timestamp.into_nanos() as f64 * full_factor) as i64;
        }
    }

    impl DataSource for Arc<FakeDataSource> {
        fn capture(&self) -> (zx::MonotonicInstant, zx::MemoryStall) {
            *self.result.lock()
        }

        fn watch(
            &self,
            _kind: zx::MemoryStallKind,
            _threshold: zx::MonotonicDuration,
            _window: zx::MonotonicDuration,
        ) -> Result<zx::Event, zx::Status> {
            unimplemented!()
        }
    }

    #[test]
    fn test_query() {
        let data_source = Arc::new(FakeDataSource::default());
        let psi_provider = PsiProvider::new_for_test(data_source.clone());

        // Simulate the following "some" memory pressure curve:
        //  - 60% for the first 240 seconds.
        //  - 72% for the next 50 seconds.
        //  - 90% for the last 10 seconds.
        // And simulate half of it for "full", i.e. 30%, 36% and 45%.
        data_source.increment(zx::Duration::from_seconds(240), 0.60, 0.30);
        psi_provider.update_history();
        data_source.increment(zx::Duration::from_seconds(50), 0.72, 0.36);
        psi_provider.update_history();
        data_source.increment(zx::Duration::from_seconds(10), 0.90, 0.45);
        psi_provider.update_history();

        // Verify that the averages match the expectations, using string comparisons for rounding.
        let result = psi_provider.query();
        let result_some = result.some.as_ref().unwrap();
        let result_full = result.full.as_ref().unwrap();
        assert!(
            format!("{:.2}", result_some.avg10.unwrap()) == "0.90"
                && format!("{:.2}", result_some.avg60.unwrap()) == "0.75"
                && format!("{:.2}", result_some.avg300.unwrap()) == "0.63"
                && format!("{:.2}", result_full.avg10.unwrap()) == "0.45"
                && format!("{:.2}", result_full.avg60.unwrap()) == "0.38"
                && format!("{:.2}", result_full.avg300.unwrap()) == "0.32",
            "{result:?}"
        );
    }
}
