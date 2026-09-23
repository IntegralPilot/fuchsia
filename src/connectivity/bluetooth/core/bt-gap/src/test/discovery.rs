// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::host_device::HostDevice;
use crate::services::access;
use crate::{host_device, host_dispatcher};
use fidl_fuchsia_bluetooth_sys::AccessMarker;
use fuchsia_async as fasync;
use fuchsia_bluetooth::types::HostId;
use fuchsia_sync::RwLock;
use futures::stream::FuturesUnordered;
use futures::Future;
use proptest::prelude::*;
use std::collections::HashMap;
use std::fmt::Debug;
use std::iter;
use std::pin::{pin, Pin};
use std::sync::Arc;

// Maximum number of mock access.fidl clients we simulate.
// For a more comprehensive test, increase this number.
// For a faster running test, decrease this number.
const MAX_NUM_MOCK_CLIENTS: u64 = 4;

#[derive(Clone, Debug)]
enum Event {
    ToggleDiscovery(u64),
    Execute,
    RemoveHost,
}

fn stream_events(client_no: u64) -> Vec<Event> {
    // For each client, we add two ToggleDiscovery events to the execution sequence
    // The first will start discovery, the second will stop discovery
    vec![Event::ToggleDiscovery(client_no), Event::ToggleDiscovery(client_no)]
}

/// A generated strategy is a Vector of Events, i.e. an execution sequence.
/// The number of clients is a random integer between 0 and max_num_clients.
/// An execution sequence is a random permutation of 4 events per client.
/// These 4 events are 2 ToggleDiscovery(client_no) events and 2 Execute events.
fn execution_sequences(max_num_clients: u64) -> impl Strategy<Value = Vec<Event>> {
    fn generate_events(num_clients: u64) -> Vec<Event> {
        let mut events =
            (0..num_clients).flat_map(|client_no| stream_events(client_no)).collect::<Vec<_>>();
        events.extend(iter::repeat(Event::Execute).take((num_clients * 2) as usize));
        events.push(Event::RemoveHost);
        events
    }

    (0..max_num_clients + 1).prop_map(generate_events).prop_shuffle()
}

fn run_access_and_host<F, T, U>(
    executor: &mut fuchsia_async::TestExecutor,
    access_sessions: &mut FuturesUnordered<T>,
    host_task: &mut Pin<Box<F>>,
    update_test_host_info_fut: &mut Pin<Box<U>>,
    host: host_device::HostDevice,
) -> bool
where
    T: futures::Future,
    F: futures::Future,
    U: futures::Future,
{
    for _ in 0..4 {
        // Run Access server sessions
        for mut access_fut in Pin::new(&mut *access_sessions).iter_pin_mut() {
            let _ = executor.run_until_stalled(&mut access_fut);
        }

        // Run mock host server.
        let _ = executor.run_until_stalled(host_task);
        let _ = executor.run_until_stalled(update_test_host_info_fut);
    }

    let _ = executor.run_until_stalled(update_test_host_info_fut);
    host.info().discovering
}

fn run_watch_host_info(host_device: &HostDevice) -> impl Future<Output = ()> {
    let host = host_device.clone();
    async move {
        loop {
            if let Err(_e) = host.clone().refresh_test_host_info().await {
                return;
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig{
        // Disable persistence to avoid the warning for not running in the
        // source code directory (since we're running on a Fuchsia target)
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_discovery_invariants(execution in execution_sequences(MAX_NUM_MOCK_CLIENTS)) {
        let mut executor = fasync::TestExecutor::new();
        // Maps {client no. -> discovery session token}
        let mut discovery_sessions = HashMap::new();
        // Maps {client no. -> access proxy}
        let mut access_proxies = HashMap::new();
        // For the collection of access::run futures
        let mut access_sessions = FuturesUnordered::new();
        let hd = host_dispatcher::test::make_simple_test_dispatcher();

        // Add mock host 1 to dispatcher and make active
        let add_mock_host_fut_1 = host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(1), &hd);
        let mut add_mock_host_fut_1 = pin!(add_mock_host_fut_1);
        let (host_stream_1, host_device_1, _gatt_server_1, _bonding) = executor.run_singlethreaded(&mut add_mock_host_fut_1).unwrap();
        let host_info_1 = Arc::new(RwLock::new(host_device_1.info()));
        hd.set_active_host(host_device_1.id())?;
        let mut active_host = 1;

        let mut host_server_task_1 =
            Box::pin(host_device::test::run_discovery_host_server(host_stream_1, host_info_1));
        let mut host_update_info_fut_1 = Box::pin(run_watch_host_info(&host_device_1));

        // Add mock host 2 to dispatcher but do NOT make active.
        let add_mock_host_fut_2 = host_dispatcher::test::create_and_add_test_host_to_dispatcher(HostId(2), &hd);
        let mut add_mock_host_fut_2 = pin!(add_mock_host_fut_2);
        let (host_stream_2, host_device_2, _gatt_server_2, _bonding) = executor.run_singlethreaded(&mut add_mock_host_fut_2).unwrap();
        let host_info_2 = Arc::new(RwLock::new(host_device_2.info()));

        let mut host_server_task_2 =
            Box::pin(host_device::test::run_discovery_host_server(host_stream_2, host_info_2));
        let mut host_update_info_fut_2 = Box::pin(run_watch_host_info(&host_device_2));

        for (num, event) in execution.into_iter().enumerate() {
            match event {
                Event::ToggleDiscovery(client_num) => {
                    match discovery_sessions.remove(&client_num) {
                        // send StartDiscovery request
                        None => {
                            let (access_proxy, access_stream) =
                                fidl::endpoints::create_proxy_and_stream::<AccessMarker>();
                            access_sessions.push(access::run(hd.clone(), access_stream));

                            let (discovery_session, discovery_session_server) =
                                fidl::endpoints::create_proxy();

                            let _ = executor.run_until_stalled(
                                &mut access_proxy.start_discovery(discovery_session_server),
                            );

                            assert!(discovery_sessions.insert(client_num, discovery_session).is_none());
                            let _ = access_proxies.insert(client_num, access_proxy);
                        }

                        // drop existing discovery session
                        Some(discovery_session) => {
                            drop(discovery_session);
                        }
                    }
                }
                Event::RemoveHost => {
                            let mut rm_device = pin!(hd.rm_device("/dev/host1"));
                            let _ = executor.run_until_stalled(&mut rm_device);
                            active_host = 2;
                }
                Event::Execute => {
                     let is_discovering_1 = run_access_and_host(
                            &mut executor,
                            &mut access_sessions,
                            &mut host_server_task_1,
                            &mut host_update_info_fut_1,
                            host_device_1.clone(),
                    );
                    let is_discovering_2 = run_access_and_host(
                            &mut executor,
                            &mut access_sessions,
                            &mut host_server_task_2,
                            &mut host_update_info_fut_2,
                            host_device_2.clone(),
                    );

                    // INVARIANT:
                    // If discovery_sessions is nonempty, at least one client holds a discovery
                    // session token, so host should be discovering. Otherwise, host should not
                    // be discovering.

                    // We test our invariant during Execute events, after driving the Access
                    // and host futures, to ensure there are no pending requests, the
                    // processing of which may affect the system's state.
                    if active_host == 1 {
                        prop_assert_eq!(is_discovering_1, !discovery_sessions.is_empty(), "step {} host 1 sessions {}", num, discovery_sessions.len());
                        prop_assert_eq!(is_discovering_2, false, "step {} host 2 is active when not", num);
                    } else {
                        prop_assert_eq!(is_discovering_1, false, "step {} host 1 is active after remove", num);
                        prop_assert_eq!(is_discovering_2, !discovery_sessions.is_empty(), "step {} host 2 sessions {}", num, discovery_sessions.len());
                    }
                }
            }
        }

        // Drop remaining discovery sessions and ensure host stops discovery
        drop(discovery_sessions);
        let is_discovering = run_access_and_host(
            &mut executor,
            &mut access_sessions,
            &mut host_server_task_1,
            &mut host_update_info_fut_1,
            host_device_1.clone(),
        );
        prop_assert!(!is_discovering);
        let is_discovering = run_access_and_host(
            &mut executor,
            &mut access_sessions,
            &mut host_server_task_2,
            &mut host_update_info_fut_2,
            host_device_2.clone(),
        );
        prop_assert!(!is_discovering);
    }
}
