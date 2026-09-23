// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use cache_manager_config_lib::Config;
use fuchsia_component::{self, client as fclient};
use log::*;
use std::process;
use {
    component_framework_cache_metrics_registry as metrics, fidl_fuchsia_metrics,
    fidl_fuchsia_sys2 as fsys, fuchsia_async as fasync,
};

#[fuchsia::main(logging_tags=["cache_manager"])]
async fn main() -> Result<(), Error> {
    info!("cache manager started");
    let config = Config::take_from_startup_handle();
    if config.cache_clearing_threshold > 100 {
        error!(
            "cache clearing threshold is too high, must be <= 100 but it is {}",
            config.cache_clearing_threshold
        );
        process::exit(1);
    }
    info!(
        "cache will be cleared when storage passes {}% capacity",
        config.cache_clearing_threshold
    );
    info!("checking storage every {} milliseconds", config.storage_checking_frequency);

    let storage_admin = fclient::connect_to_protocol::<fsys::StorageAdminMarker>()
        .context("failed opening storage admin")?;

    // Setup telemetry module
    let cobalt_logger = setup_cobalt_proxy().await;

    monitor_storage(&storage_admin, config, cobalt_logger).await;
    Ok(())
}

#[derive(PartialEq, Debug)]
struct StorageState {
    total_bytes: u64,
    used_bytes: u64,
}

impl StorageState {
    fn percent_used(&self) -> u64 {
        if self.total_bytes > 0 { self.used_bytes * 100 / self.total_bytes } else { 0 }
    }
}

async fn monitor_storage(
    storage_admin: &fsys::StorageAdminProxy,
    config: Config,
    cobalt_logger: Option<fidl_fuchsia_metrics::MetricEventLoggerProxy>,
) {
    // Sleep for the check interval, then see if we're over the clearing threshold.
    // If we are over the threshold, clear the cache. This panics if we lose the
    // connect to the StorageAdminProtocol.

    info!(
        "cache will be cleared when storage passes {}% capacity",
        config.cache_clearing_threshold
    );
    info!("checking storage every {} milliseconds", config.storage_checking_frequency);

    loop {
        fasync::Timer::new(std::time::Duration::from_millis(config.storage_checking_frequency))
            .await;
        let storage_state = {
            match get_storage_utilization(&storage_admin).await {
                Ok(utilization) => {
                    if utilization.percent_used() > 100 {
                        warn!("storage utlization is above 100%, clearing storage.");
                    }
                    utilization
                }
                Err(e) => match e.downcast_ref::<fidl::Error>() {
                    Some(fidl::Error::ClientChannelClosed { .. }) => {
                        panic!(
                            "cache manager's storage admin channel closed unexpectedly, \
                            is component manager dead?"
                        );
                    }
                    _ => {
                        error!("failed getting cache utilization, will try again later: {:?}", e);
                        continue;
                    }
                },
            }
        };

        // Not enough storage is used, sleep and wait for changes
        if storage_state.percent_used() < config.cache_clearing_threshold {
            continue;
        }

        // Clear the cache
        info!(
            "storage utilization is at {}%, which is above our threshold of {}%, beginning to clear cache storage",
            storage_state.percent_used(),
            config.cache_clearing_threshold
        );

        match clear_cache_storage(&storage_admin).await {
            Err(e) => match e.downcast_ref::<fidl::Error>() {
                Some(fidl::Error::ClientChannelClosed { .. }) => {
                    log_cobalt_occurence(
                        &cobalt_logger,
                        metrics::CACHE_EVICTION_METRIC_ID,
                        &[metrics::CacheEvictionMetricDimensionResult::FailedChannelClosed as u32],
                    )
                    .await;
                    panic!(
                        "cache manager's storage admin channel closed while clearing storage \
                        is component manager dead?"
                    );
                }
                _ => {
                    error!("non-fatal error while clearing cache: {:?}", e);
                    continue;
                }
            },
            _ => {}
        }

        let storage_state_after = match get_storage_utilization(&storage_admin).await {
            Err(e) => match e.downcast_ref::<fidl::Error>() {
                Some(fidl::Error::ClientChannelClosed { .. }) => {
                    panic!(
                        "cache manager's storage admin channel closed while checking utlization \
                            after cache clearing, is component manager dead?"
                    );
                }
                _ => {
                    error!("non-fatal getting storage utlization {:?}", e);
                    continue;
                }
            },
            Ok(u) => u,
        };

        let mut unconditional_success = true;

        if storage_state_after.percent_used() > config.cache_clearing_threshold {
            warn!(
                "storage usage still exceeds threshold after cache clearing, used_bytes={} total_bytes={}",
                storage_state.used_bytes, storage_state.total_bytes
            );
            unconditional_success = false;
        }

        if storage_state_after.percent_used() >= storage_state.percent_used() {
            warn!("cache manager did not reduce storage pressure");
            unconditional_success = false;
        }

        let metric_dimensions = if unconditional_success {
            vec![metrics::CacheEvictionMetricDimensionResult::Success as u32]
        } else {
            vec![metrics::CacheEvictionMetricDimensionResult::SuccessWithCaveats as u32]
        };
        log_cobalt_occurence(&cobalt_logger, metrics::CACHE_EVICTION_METRIC_ID, &metric_dimensions)
            .await;
    }
}

/// Check the current cache storage utilization. If no components are using cache, the utilization
/// reported by the filesystem is not checked and utlization is reported as zero.
async fn get_storage_utilization(
    storage_admin: &fsys::StorageAdminProxy,
) -> Result<StorageState, Error> {
    let utilization = match storage_admin.get_status().await? {
        Ok(u) => u,
        Err(e) => {
            return Err(format_err!(
                "RPC to get storage status succeeded, but server returned an error: {:?}",
                e
            ));
        }
    };

    Ok(StorageState {
        total_bytes: utilization.total_size.unwrap(),
        used_bytes: utilization.used_size.unwrap(),
    })
}

/// Tries to delete the cache for all components. Failures for individual components are ignored,
/// but if `storage_admin` is closed that is reported as the `fidl::Error::ClientChannelClosed`
/// that it is.
async fn clear_cache_storage(storage_admin: &fsys::StorageAdminProxy) -> Result<(), Error> {
    storage_admin
        .delete_all_storage_contents()
        .await?
        .map_err(|err| format_err!("protocol error clearing cache: {:?}", err))
}

async fn setup_cobalt_proxy() -> Option<fidl_fuchsia_metrics::MetricEventLoggerProxy> {
    async fn setup_proxy_internal() -> Result<fidl_fuchsia_metrics::MetricEventLoggerProxy, Error> {
        let cobalt_1dot1_svc = fuchsia_component::client::connect_to_protocol::<
            fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker,
        >()
        .context("failed to connect to metrics service")?;

        let (cobalt_1dot1_proxy, cobalt_1dot1_server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();

        let project_spec = fidl_fuchsia_metrics::ProjectSpec {
            customer_id: Some(metrics::CUSTOMER_ID),
            project_id: Some(metrics::PROJECT_ID),
            ..Default::default()
        };

        match cobalt_1dot1_svc.create_metric_event_logger(&project_spec, cobalt_1dot1_server).await
        {
            Ok(_) => Ok(cobalt_1dot1_proxy),
            Err(err) => Err(format_err!("failed to create metrics event logger: {:?}", err)),
        }
    }

    match setup_proxy_internal().await {
        Ok(proxy) => Some(proxy),
        Err(err) => {
            warn!("Cobalt service unavailable, will discard all metrics: {}", err);
            // This will only happen if Cobalt is very broken in some way. Using the disconnected
            // proxy will result metrics failing to send, but it's preferable to run rather than
            // panicking here.
            None
        }
    }
}

async fn log_cobalt_occurence(
    cobalt_logger: &Option<fidl_fuchsia_metrics::MetricEventLoggerProxy>,
    metric_id: u32,
    event_codes: &[u32],
) {
    if let Some(c) = cobalt_logger {
        if let Err(e) = c.log_occurrence(metric_id, 1, event_codes).await {
            warn!("Failed to log metrics: {:?}", e)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor_storage;
    use cache_manager_config_lib::Config;
    use fidl::endpoints::{ClientEnd, ServerEnd};
    use fidl_fuchsia_metrics::{MetricEvent, MetricEventLoggerRequest, MetricEventPayload};
    use fidl_fuchsia_sys2 as fsys;
    use fuchsia_async::{self as fasync, MonotonicDuration, TestExecutor};
    use futures::channel::mpsc::{self as mpsc, UnboundedReceiver};
    use futures::{StreamExt, TryStreamExt};
    use std::future::Future;
    use std::pin::Pin;
    struct FakeStorageServer {
        storage_statuses: Vec<fsys::StorageStatus>,
        chan: ServerEnd<fsys::StorageAdminMarker>,
    }

    #[derive(Debug, PartialEq)]
    enum CallType {
        Status,
        Delete,
    }

    impl FakeStorageServer {
        /// Run the fake server. The server sends monikers it receives it storage deletion requests
        /// over the `moniker_channel`.
        pub fn run_server(
            mut self,
            call_chan: mpsc::UnboundedSender<CallType>,
        ) -> Pin<Box<impl Future<Output = ()>>> {
            let server = async move {
                let mut req_stream = self.chan.into_stream();
                while let Ok(request) = req_stream.try_next().await {
                    match request {
                        Some(fsys::StorageAdminRequest::DeleteAllStorageContents { responder }) => {
                            call_chan.unbounded_send(CallType::Delete).unwrap();
                            let _ = responder.send(Ok(()));
                        }
                        Some(fsys::StorageAdminRequest::GetStatus { responder }) => {
                            call_chan.unbounded_send(CallType::Status).unwrap();
                            // note that we can panic here, but that is okay because if more
                            // status values are requested than expect that is also an error
                            let status = self.storage_statuses.remove(0);
                            let _ = responder.send(Ok(&status));
                        }
                        None => return,
                        _ => panic!("unexpected call not supported by fake server"),
                    }
                }
            };
            Box::pin(server)
        }
    }

    fn common_setup(
        server: Option<FakeStorageServer>,
        client: ClientEnd<fsys::StorageAdminMarker>,
    ) -> (
        UnboundedReceiver<CallType>,
        TestExecutor,
        MonotonicDuration,
        fsys::StorageAdminProxy,
        Config,
        fidl_fuchsia_metrics::MetricEventLoggerProxy,
        fidl_fuchsia_metrics::MetricEventLoggerRequestStream,
    ) {
        let (calls_tx, calls_rx) = futures::channel::mpsc::unbounded::<CallType>();
        let exec = TestExecutor::new_with_fake_time();
        let time_step = MonotonicDuration::from_millis(5000);
        let config = Config {
            cache_clearing_threshold: 20,
            storage_checking_frequency: time_step.clone().into_millis().try_into().unwrap(),
        };
        if let Some(server) = server {
            fasync::Task::spawn(server.run_server(calls_tx)).detach();
        }
        let client = client.into_proxy();
        let (cobalt_client, cobalt_events) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_metrics::MetricEventLoggerMarker,
        >();
        (calls_rx, exec, time_step, client, config, cobalt_client, cobalt_events)
    }

    /// Advance the TestExecutor by |time_step| and wake expired timers.
    fn advance_time_and_wake(exec: &mut TestExecutor, time_step: &MonotonicDuration) {
        let new_time = fasync::MonotonicInstant::from_nanos(
            exec.now().into_nanos() + time_step.clone().into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();
    }

    /// Continually execute the future and respond to any incoming Cobalt request with Ok, returning
    /// any logged Cobalt events.
    fn drain_cobalt_events(
        exec: &mut TestExecutor,
        main_fut: &mut (impl Future + Unpin),
        cobalt_stream: &mut fidl_fuchsia_metrics::MetricEventLoggerRequestStream,
    ) -> Vec<MetricEvent> {
        let mut cobalt_events = vec![];
        let mut made_progress = true;
        while made_progress {
            let _result = exec.run_until_stalled(main_fut);
            made_progress = false;
            while let std::task::Poll::Ready(Some(Ok(req))) =
                exec.run_until_stalled(&mut cobalt_stream.next())
            {
                match req {
                    MetricEventLoggerRequest::LogOccurrence {
                        metric_id,
                        count,
                        event_codes,
                        responder,
                    } => {
                        assert!(responder.send(Ok(())).is_ok());
                        cobalt_events.push(MetricEvent {
                            metric_id,
                            event_codes,
                            payload: MetricEventPayload::Count(count),
                        })
                    }
                    _ => panic!("unexpcted metric type"),
                };

                made_progress = true;
            }
        }
        cobalt_events
    }

    #[test]
    fn test_typical_case() {
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fsys::StorageAdminMarker>();

        // Create a take storage server with the canned call responses for
        // GetStatus that we want.
        let server = FakeStorageServer {
            storage_statuses: vec![
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(10),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(50),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(0),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(0),
                    ..Default::default()
                },
            ],
            chan: server_end,
        };

        let (mut calls_rx, mut exec, time_step, client, config, cobalt_client, mut cobalt_stream) =
            common_setup(Some(server), client_end);
        let mut monitor = Box::pin(monitor_storage(&client, config, Some(cobalt_client)));
        let _ = exec.run_until_stalled(&mut monitor);

        // We expect no query sent to the capability provider since it sleeps first
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));

        // Move forward to the first check
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);

        // We expect a status check
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Since the reported usage is below the threshold, the monitor should do nothing.
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));

        // Move forward to the next check
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);

        // Expect a check, where we'll report we're above the cache clearing threshold
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Expect that the monitor tries to clear storage
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Delete)));

        // Monitor checks after clearing storage
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Check for the Cobalt metrics
        let cobalt_metrics = drain_cobalt_events(&mut exec, &mut monitor, &mut cobalt_stream);
        assert_eq!(cobalt_metrics.len(), 1);
        assert_eq!(cobalt_metrics[0].metric_id, metrics::CACHE_EVICTION_METRIC_ID);
        assert_eq!(
            cobalt_metrics[0].event_codes,
            vec![metrics::CacheEvictionMetricDimensionResult::Success as u32]
        );
        assert_eq!(cobalt_metrics[0].payload, MetricEventPayload::Count(1));

        // No call is expected until the timer goes off
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));

        // advance time
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);

        // Monitor checks again and we'll respond that usage is below the threshold
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // There should be no call until the next check
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));
    }

    #[test]
    /// Test the scenario where clearing storage doesn't lower utilization below the clearing
    /// threshold. In this case we expect the call behavior to be the same as "normal" operrations.
    fn test_utilization_stays_above_threshold() {
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fsys::StorageAdminMarker>();

        // Create a take storage server with the canned call responses for
        // GetStatus that we want.
        let server = FakeStorageServer {
            storage_statuses: vec![
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(10),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(50),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(50),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(50),
                    ..Default::default()
                },
                fsys::StorageStatus {
                    total_size: Some(100),
                    used_size: Some(50),
                    ..Default::default()
                },
            ],
            chan: server_end,
        };

        let (mut calls_rx, mut exec, time_step, client, config, cobalt_client, mut cobalt_stream) =
            common_setup(Some(server), client_end);
        let mut monitor = Box::pin(monitor_storage(&client, config, Some(cobalt_client)));
        let _ = exec.run_until_stalled(&mut monitor);

        // We expect no query sent to the capability provider since it sleeps first
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));

        // Move forward to the first check
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);

        // We expect a status check
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Since the reported usage is below the threshold, the monitor should do nothing.
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));

        // Move forward to the next check
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);

        // Expect a check, where we'll report we're above the cache clearing threshold
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Expect that the monitor tries to clear storage
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Delete)));

        // Monitor checks after clearing storage
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Check for the Cobalt metrics
        let cobalt_metrics = drain_cobalt_events(&mut exec, &mut monitor, &mut cobalt_stream);
        assert_eq!(cobalt_metrics.len(), 1);
        assert_eq!(cobalt_metrics[0].metric_id, metrics::CACHE_EVICTION_METRIC_ID);
        assert_eq!(
            cobalt_metrics[0].event_codes,
            vec![metrics::CacheEvictionMetricDimensionResult::SuccessWithCaveats as u32]
        );
        assert_eq!(cobalt_metrics[0].payload, MetricEventPayload::Count(1));

        // No call is expected until the timer goes off
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));

        // advance time
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);

        // Monitor checks again and we'll respond that usage is below the threshold
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // Expect that the monitor tries to clear storage again on the next run
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Delete)));

        // Monitor checks after clearing storage
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Ok(Some(CallType::Status)));

        // There should be no call until the next check
        assert_eq!(calls_rx.try_next().map_err(|_| ()), Err(()));
    }

    #[test]
    #[should_panic]
    fn test_channel_closure_panics() {
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fsys::StorageAdminMarker>();

        let (mut _calls_rx, mut exec, time_step, client, config, cobalt_client, mut _cobalt_stream) =
            common_setup(None, client_end);
        let mut monitor = Box::pin(monitor_storage(&client, config, Some(cobalt_client)));
        let _ = exec.run_until_stalled(&mut monitor);

        drop(server_end);

        // Move forward to the first check
        advance_time_and_wake(&mut exec, &time_step);
        let _ = exec.run_until_stalled(&mut monitor);
    }
}
