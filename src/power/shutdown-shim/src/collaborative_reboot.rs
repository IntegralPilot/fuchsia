// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An implementation of collaborative reboot for Fuchsia.
//!
//! Collaborative reboot is a mechanism that allows multiple actors to work
//! together to schedule a device reboot at a time that avoids user disruption.
//!
//! Scheduler actors express their desire to reboot the device by issuing
//! `ScheduleReboot` requests. While the Initiator actor expresses when it is an
//! appropriate time to reboot the device by issuing a `PerformPendingReboot`
//! request.

use fidl_fuchsia_power::{
    CollaborativeRebootInitiatorPerformPendingRebootResponse as PerformPendingRebootResponse,
    CollaborativeRebootInitiatorRequest as InitiatorRequest,
    CollaborativeRebootInitiatorRequestStream as InitiatorRequestStream,
};
use fidl_fuchsia_power_internal::{
    CollaborativeRebootReason as Reason, CollaborativeRebootSchedulerRequest as SchedulerRequest,
    CollaborativeRebootSchedulerRequestStream as SchedulerRequestStream,
};
use fuchsia_async::OnSignals;
use fuchsia_inspect::NumericProperty;
use fuchsia_sync::Mutex;
use futures::{StreamExt, TryStreamExt};
use std::sync::Arc;

/// The signals that can be used to cancel a scheduled collaborative reboot.
const CANCELLATION_SIGNALS: zx::Signals =
    zx::Signals::USER_ALL.union(zx::Signals::OBJECT_PEER_CLOSED);

/// Construct a new [`State`] and it's paired [`Cancellations`].
pub(super) fn new(inspector: &fuchsia_inspect::Inspector) -> (State, Cancellations) {
    let (cancellation_sender, cancellation_receiver) = futures::channel::mpsc::unbounded();
    let cr_node = inspector.root().create_child("CollaborativeReboot");
    let scheduled_requests_node = cr_node.create_child("ScheduledRequests");

    let scheduled_requests: Arc<Mutex<ScheduledRequests>> =
        Arc::new(Mutex::new(ScheduledRequests::new(&scheduled_requests_node)));
    (
        State {
            scheduled_requests: scheduled_requests.clone(),
            cancellation_sender,
            _inspect_nodes: [cr_node, scheduled_requests_node],
        },
        Cancellations { scheduled_requests, cancellation_receiver },
    )
}

#[derive(Debug)]
/// Collaborative Reboot State.
pub(super) struct State {
    /// The current outstanding requests for collaborative reboot.
    ///
    /// Also held by [`Cancellations`].
    scheduled_requests: Arc<Mutex<ScheduledRequests>>,

    /// The sender of cancellation signals.
    ///
    /// The receiver half is held by [`Cancellations`].
    cancellation_sender: futures::channel::mpsc::UnboundedSender<Cancel>,

    /// A container to hold the Inspect nodes for collaborative reboot.
    ///
    /// These nodes need to retained, as dropping them would delete the
    /// underlying Inspect data.
    _inspect_nodes: [fuchsia_inspect::types::Node; 2],
}

impl State {
    /// Handle requests from a [`CollaborativeRebootScheduler`].
    pub(super) async fn handle_scheduler_requests(&self, mut stream: SchedulerRequestStream) {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                SchedulerRequest::ScheduleReboot { reason, cancel, responder } => {
                    {
                        let mut scheduled_requests = self.scheduled_requests.lock();
                        scheduled_requests.schedule(reason);
                        println!(
                            "[shutdown-shim] Collaborative reboot scheduled for reason: \
                            {reason:?}. Current scheduled requests: {scheduled_requests}"
                        );
                    }
                    if let Some(cancel) = cancel {
                        self.cancellation_sender
                            .unbounded_send(Cancel { signal: cancel, reason })
                            .expect("receiver should not close");
                    }
                    match responder.send() {
                        Ok(()) => {}
                        Err(e) => {
                            eprintln!(
                                "[shutdown-shim] Failed to respond to 'ScheduleReboot': {e:?}"
                            );
                            // Returning closes the connection.
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Handle requests from a [`CollaborativeRebootInitiator`].
    pub(super) async fn handle_initiator_requests<R: RebootActuator>(
        &self,
        mut stream: InitiatorRequestStream,
        actuator: &R,
    ) {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                InitiatorRequest::PerformPendingReboot { responder } => {
                    let reboot_reasons = {
                        let scheduled_requests = self.scheduled_requests.lock();
                        println!(
                            "[shutdown-shim] Asked to perform collaborative reboot. \
                            Current scheduled requests: {scheduled_requests}"
                        );
                        scheduled_requests.list_reasons()
                    };

                    let rebooting = !reboot_reasons.is_empty();

                    if rebooting {
                        println!("[shutdown-shim] Performing collaborative reboot ...");
                        match actuator.perform_reboot(reboot_reasons).await {
                            Ok(()) | Err(zx::Status::ALREADY_EXISTS) => {}
                            Err(status) => {
                                eprintln!("[shutdown-shim] Failed to perform reboot: {status:?}");
                                // Returning closes the connection.
                                return;
                            }
                        }
                    }

                    match responder.send(&PerformPendingRebootResponse {
                        rebooting: Some(rebooting),
                        __source_breaking: fidl::marker::SourceBreaking,
                    }) {
                        Ok(()) => {}
                        Err(e) => {
                            eprintln!(
                                "[shutdown-shim] Failed to respond to 'PerformPendingReboot': {e:?}"
                            );
                            // Returning closes the connection.
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Holds cancellation signals for collaborative reboot.
pub(super) struct Cancellations {
    /// The current outstanding requests for collaborative reboot.
    ///
    /// Also held by [`State`].
    scheduled_requests: Arc<Mutex<ScheduledRequests>>,

    /// The receiver of cancellation signals.
    ///
    /// The sender half is held by [`State`].
    cancellation_receiver: futures::channel::mpsc::UnboundedReceiver<Cancel>,
}

impl Cancellations {
    /// Drives the cancellation of scheduled requests.
    ///
    /// The returned future will only exit if the corresponding [`State`] is
    /// dropped.
    pub(super) async fn run(self) -> () {
        let Self { scheduled_requests, cancellation_receiver } = self;
        cancellation_receiver
            .for_each_concurrent(None, |Cancel { reason, signal }| {
                let scheduled_requests = scheduled_requests.clone();
                async move {
                    // Note: Panic if we observe an error, as all `Err` statuses
                    // represent programming errors. See the documentation for
                    // `zx_object_wait_one`:
                    // https://fuchsia.dev/reference/syscalls/object_wait_one.
                    let _signals = OnSignals::new(signal, CANCELLATION_SIGNALS)
                        .await
                        .expect("failed to wait for signals on eventpair");
                    {
                        let mut scheduled_requests = scheduled_requests.lock();
                        scheduled_requests.cancel(reason);
                        println!(
                            "[shutdown-shim] Collaborative reboot canceled for reason: {reason:?}. \
                            Current scheduled requests: {scheduled_requests}"
                        );
                    }
                }
            })
            .await
    }
}

// A tuple of the cancellation signal, and the reason being canceled.
struct Cancel {
    reason: Reason,
    signal: zx::EventPair,
}

/// A reference count of the scheduled requests, broken down by reason.
#[derive(Debug)]
struct ScheduledRequests {
    system_update: usize,
    netstack_migration: usize,
    // Inspect properties to report the counters from above.
    system_update_inspect_prop: fuchsia_inspect::types::UintProperty,
    netstack_migration_inspect_prop: fuchsia_inspect::types::UintProperty,
}

impl ScheduledRequests {
    fn new(inspect_node: &fuchsia_inspect::types::Node) -> Self {
        Self {
            system_update: 0,
            netstack_migration: 0,
            system_update_inspect_prop: inspect_node.create_uint("SystemUpdate", 0),
            netstack_migration_inspect_prop: inspect_node.create_uint("NetstackMigration", 0),
        }
    }

    fn schedule(&mut self, reason: Reason) {
        let (rc, inspect_prop) = match reason {
            Reason::SystemUpdate => (&mut self.system_update, &self.system_update_inspect_prop),
            Reason::NetstackMigration => {
                (&mut self.netstack_migration, &self.netstack_migration_inspect_prop)
            }
        };
        *rc = rc.saturating_add(1);
        inspect_prop.add(1);
    }

    fn cancel(&mut self, reason: Reason) {
        let (rc, inspect_prop) = match reason {
            Reason::SystemUpdate => (&mut self.system_update, &self.system_update_inspect_prop),
            Reason::NetstackMigration => {
                (&mut self.netstack_migration, &self.netstack_migration_inspect_prop)
            }
        };
        *rc = rc.saturating_sub(1);
        inspect_prop.subtract(1);
    }

    fn list_reasons(&self) -> Vec<Reason> {
        let Self {
            system_update,
            netstack_migration,
            system_update_inspect_prop: _,
            netstack_migration_inspect_prop: _,
        } = self;
        let mut reasons = Vec::new();
        if *system_update != 0 {
            reasons.push(Reason::SystemUpdate);
        }
        if *netstack_migration != 0 {
            reasons.push(Reason::NetstackMigration);
        }
        reasons
    }
}

impl std::fmt::Display for ScheduledRequests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            system_update,
            netstack_migration,
            system_update_inspect_prop: _,
            netstack_migration_inspect_prop: _,
        } = self;
        write!(f, "SystemUpdate:{system_update}, NetstackMigration:{netstack_migration}")
    }
}

/// A type capable of actuating reboots on behalf of collaborative reboot.
pub(super) trait RebootActuator {
    /// Perform the reboot with the given reasons.
    async fn perform_reboot(&self, reasons: Vec<Reason>) -> Result<(), zx::Status>;
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;

    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fidl::Peered;
    use fidl_fuchsia_power::CollaborativeRebootInitiatorMarker;
    use fidl_fuchsia_power_internal::CollaborativeRebootSchedulerMarker;
    use futures::FutureExt;
    use futures::future::Either;
    use futures::stream::FuturesUnordered;
    use test_case::test_case;

    #[derive(Default)]
    struct MockRebooter {
        /// Stores if `perform_reboot` has been called, and with what arguments.
        ///
        /// Wrapped in a `RefCell` to allow interior mutability. Because the
        /// tests are single threaded, this isn't problematic.
        reasons: RefCell<Option<Vec<Reason>>>,
    }

    impl RebootActuator for MockRebooter {
        async fn perform_reboot(&self, reasons: Vec<Reason>) -> Result<(), zx::Status> {
            let original_reasons = self.reasons.borrow_mut().replace(reasons);
            // The mock only expects to have `perform_reboot` called once.
            assert_eq!(None, original_reasons);
            Ok(())
        }
    }

    #[test_case(vec![] => false; "no_pending_reboot")]
    #[test_case(vec![Reason::SystemUpdate] => true; "system_update")]
    #[test_case(vec![Reason::NetstackMigration] => true; "netstack_migration")]
    #[test_case(vec![Reason::SystemUpdate, Reason::NetstackMigration] => true; "different_reasons")]
    #[test_case(vec![Reason::SystemUpdate, Reason::SystemUpdate] => true; "same_reasons")]
    #[fuchsia::test]
    async fn collaborative_reboot(mut reasons: Vec<Reason>) -> bool {
        // Note: this test doesn't exercise cancellations.
        let inspector =
            fuchsia_inspect::Inspector::new(fuchsia_inspect::InspectorConfig::default());
        let (state, _cancellations) = new(&inspector);

        let (scheduler_client, scheduler_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootSchedulerMarker>();
        let scheduler = scheduler_client.into_proxy();
        let (initiator_client, initiator_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootInitiatorMarker>();
        let initiator = initiator_client.into_proxy();

        // Schedule reboots for each reason (and drive the server
        // implementation).
        let schedule_server_fut = state.handle_scheduler_requests(scheduler_request_stream).fuse();
        futures::pin_mut!(schedule_server_fut);
        for reason in &reasons {
            futures::select!(
                result = scheduler.schedule_reboot(*reason, None).fuse() => {
                    result.expect("failed to schedule reboot");
                },
                () = schedule_server_fut => {
                    unreachable!("The `Scheduler` protocol worker shouldn't exit");
                }
            );
        }

        // Initiate the reboot (and drive the server implementation).
        let mock = MockRebooter::default();
        let PerformPendingRebootResponse { rebooting, __source_breaking } = futures::select!(
            result = initiator.perform_pending_reboot().fuse() => {
                result.expect("failed to initate reboot")
            },
            () = state.handle_initiator_requests(initiator_request_stream, &mock).fuse() => {
                unreachable!("The `Initiator` protocol worker shouldn't exit.");
            }
        );

        // Verify that the correct reasons were reported
        let expected_reasons = if reasons.is_empty() {
            None
        } else {
            // De-duplicate the list. If the same reason is scheduled multiple
            // times, it's only expected to be reported once.
            reasons.sort();
            reasons.dedup();
            Some(reasons)
        };
        assert_eq!(*mock.reasons.borrow(), expected_reasons);

        rebooting.expect("rebooting should be present")
    }

    // Specifies how a request should be cancelled, if at all.
    enum Cancel {
        None,
        UserSignal,
        Drop,
    }

    #[test_case(vec![(Reason::SystemUpdate, Cancel::None)] => true; "not_canceled")]
    #[test_case(vec![(Reason::SystemUpdate, Cancel::UserSignal)] => false; "canceled")]
    #[test_case(vec![(Reason::SystemUpdate, Cancel::Drop)] => false; "canceled_with_drop")]
    #[test_case(vec![
        (Reason::SystemUpdate, Cancel::UserSignal), (Reason::SystemUpdate, Cancel::None)
        ] => true; "same_reasons_only_one_canceled")]
    #[test_case(vec![
        (Reason::SystemUpdate, Cancel::UserSignal), (Reason::SystemUpdate, Cancel::UserSignal)
        ] => false; "same_reasons_both_canceled")]
    #[test_case(vec![
        (Reason::SystemUpdate, Cancel::UserSignal), (Reason::NetstackMigration, Cancel::None)
        ] => true; "different_reasons_only_one_canceled")]
    #[test_case(vec![
        (Reason::SystemUpdate, Cancel::UserSignal), (Reason::NetstackMigration, Cancel::UserSignal)
        ] => false; "different_reasons_both_canceled")]
    #[fuchsia::test]
    fn cancellation(requests: Vec<(Reason, Cancel)>) -> bool {
        // Note: This test requires partially driving the cancellation worker,
        // so we need direct access to the executor.
        let mut exec = fuchsia_async::TestExecutor::new();

        let inspector =
            fuchsia_inspect::Inspector::new(fuchsia_inspect::InspectorConfig::default());
        let (state, cancellations_worker) = new(&inspector);

        let (scheduler_client, scheduler_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootSchedulerMarker>();
        let scheduler = scheduler_client.into_proxy();
        let (initiator_client, initiator_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootInitiatorMarker>();
        let initiator = initiator_client.into_proxy();

        let mut cancellation_signals = vec![];
        // Note: We need to hold onto the uncancelled_signals, as dropping them
        // would result in the request being canceled.
        let mut uncancelled_signals = vec![];
        let mut uncancelled_reasons = vec![];

        // Schedule reboots for each reason (and drive the server
        // implementation).
        let schedule_client_fut = FuturesUnordered::new();
        for (reason, should_cancel) in requests {
            let (ep1, ep2) = zx::EventPair::create();
            schedule_client_fut.push(scheduler.schedule_reboot(reason, Some(ep2)));
            match should_cancel {
                Cancel::None => {
                    uncancelled_signals.push(ep1);
                    uncancelled_reasons.push(reason);
                }
                Cancel::UserSignal => cancellation_signals.push(ep1),
                Cancel::Drop => std::mem::drop(ep1),
            }
        }
        let schedule_client_fut = schedule_client_fut
            .for_each(|result| futures::future::ready(result.expect("failed to schedule_reboot")));
        futures::pin_mut!(schedule_client_fut);
        let schedule_server_fut = state.handle_scheduler_requests(scheduler_request_stream);
        futures::pin_mut!(schedule_server_fut);
        let schedule_fut =
            futures::future::select(schedule_client_fut, schedule_server_fut).map(|result| {
                match result {
                    Either::Left(((), _server_fut)) => {}
                    Either::Right(((), _client_fut)) => {
                        unreachable!("The `Scheduler` protocol worker shouldn't exit")
                    }
                }
            });
        futures::pin_mut!(schedule_fut);
        exec.run_singlethreaded(&mut schedule_fut);

        // Cancel some of the requests, and drive the cancellations worker
        // until it stalls (indicating it's processed all cancellations).
        for cancellation in cancellation_signals {
            cancellation
                .signal_peer(zx::Signals::NONE, zx::Signals::USER_0)
                .expect("failed to cancel reboot");
        }
        let cancellation_fut = cancellations_worker.run();
        futures::pin_mut!(cancellation_fut);
        assert_eq!(futures::task::Poll::Pending, exec.run_until_stalled(&mut cancellation_fut));

        // Initiate the reboot (and drive the server implementation).
        let mock = MockRebooter::default();
        let initiate_client_fut = initiator.perform_pending_reboot();
        futures::pin_mut!(initiate_client_fut);
        let initiate_server_fut = state.handle_initiator_requests(initiator_request_stream, &mock);
        futures::pin_mut!(initiate_server_fut);
        let initiate_fut =
            futures::future::select(initiate_client_fut, initiate_server_fut).map(|result| {
                match result {
                    Either::Left((result, _server_fut)) => {
                        result.expect("failed to initate reboot")
                    }
                    Either::Right(((), _client_fut)) => {
                        unreachable!("The `Initiator` protocol worker shouldn't exit")
                    }
                }
            });
        futures::pin_mut!(initiate_fut);
        let PerformPendingRebootResponse { rebooting, __source_breaking } =
            exec.run_singlethreaded(&mut initiate_fut);

        // Verify that the correct reasons were reported
        let expected_reasons = if uncancelled_reasons.is_empty() {
            None
        } else {
            // De-duplicate the list. If the same reason is scheduled multiple
            // times, it's only expected to be reported once.
            uncancelled_reasons.sort();
            uncancelled_reasons.dedup();
            Some(uncancelled_reasons)
        };
        assert_eq!(*mock.reasons.borrow(), expected_reasons);

        rebooting.expect("rebooting should be present")
    }

    struct MockRebooterWithError {
        /// The error status to return for each call to `perform_reboot`.
        error: zx::Status,
    }

    impl RebootActuator for MockRebooterWithError {
        async fn perform_reboot(&self, _reasons: Vec<Reason>) -> Result<(), zx::Status> {
            Err(self.error)
        }
    }

    #[test_case(zx::Status::ALREADY_EXISTS, true; "already_exists")]
    #[test_case(zx::Status::INTERNAL, false; "internal")]
    #[fuchsia::test]
    async fn reboot_error(error: zx::Status, should_succeed: bool) {
        let inspector =
            fuchsia_inspect::Inspector::new(fuchsia_inspect::InspectorConfig::default());
        let (state, _cancellations) = new(&inspector);

        let (scheduler_client, scheduler_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootSchedulerMarker>();
        let scheduler = scheduler_client.into_proxy();
        let (initiator_client, initiator_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootInitiatorMarker>();
        let initiator = initiator_client.into_proxy();

        // Schedule a reboot with an arbitrary reason (and drive the server
        // implementation).
        let schedule_server_fut = state.handle_scheduler_requests(scheduler_request_stream).fuse();
        futures::pin_mut!(schedule_server_fut);
        futures::select!(
            result = scheduler.schedule_reboot(Reason::SystemUpdate, None).fuse() => {
                result.expect("failed to schedule reboot");
            },
            () = schedule_server_fut => {
                unreachable!("The `Scheduler` protocol worker shouldn't exit");
            }
        );

        // Initiate the reboot (and drive the server implementation).
        let mock = MockRebooterWithError { error };
        if should_succeed {
            let result = futures::select!(
                result = initiator.perform_pending_reboot().fuse() => result,
                () = state.handle_initiator_requests(initiator_request_stream, &mock).fuse() => {
                    unreachable!("The `Initiator` protocol worker shouldn't exit.");
                }
            );
            assert_matches!(result, Ok(_))
        } else {
            let (result, ()) = futures::join!(
                initiator.perform_pending_reboot().fuse(),
                state.handle_initiator_requests(initiator_request_stream, &mock).fuse()
            );
            assert_matches!(result, Err(_))
        }
    }

    #[fuchsia::test]
    async fn inspect() {
        let inspector =
            fuchsia_inspect::Inspector::new(fuchsia_inspect::InspectorConfig::default());
        let (state, cancellations_worker) = new(&inspector);

        let mut cancellation_signals = vec![];
        let (scheduler_client, scheduler_request_stream) =
            fidl::endpoints::create_request_stream::<CollaborativeRebootSchedulerMarker>();
        let scheduler = scheduler_client.into_proxy();
        let schedule_server_fut = state.handle_scheduler_requests(scheduler_request_stream).fuse();
        futures::pin_mut!(schedule_server_fut);

        // At the start, expect the tree to be initialized with 0 values.
        assert_data_tree!(inspector, root: {
            "CollaborativeReboot": {
                "ScheduledRequests": {
                    "SystemUpdate": 0u64,
                    "NetstackMigration": 0u64,
                }
            }
        });

        // Schedule a `SystemUpdate` reboot, twice, and verify the inspect data.
        for count in [1u64, 2u64] {
            let (ep1, ep2) = zx::EventPair::create();
            cancellation_signals.push(ep1);
            futures::select!(
                result = scheduler.schedule_reboot(
                    Reason::SystemUpdate, Some(ep2)).fuse() => {
                        result.expect("failed to schedule reboot");
                    },
                () = schedule_server_fut => {
                    unreachable!("The `Scheduler` protocol worker shouldn't exit");
                }
            );
            assert_data_tree!(inspector, root: {
                "CollaborativeReboot": {
                    "ScheduledRequests": {
                        "SystemUpdate": count,
                        "NetstackMigration": 0u64,
                    }
                }
            });
        }

        // Schedule a `NetstackMigration` reboot, and verify the inspect data.
        futures::select!(
            result = scheduler.schedule_reboot(Reason::NetstackMigration, None).fuse() => {
                    result.expect("failed to schedule reboot");
                },
            () = schedule_server_fut => {
                unreachable!("The `Scheduler` protocol worker shouldn't exit");
            }
        );
        assert_data_tree!(inspector, root: {
            "CollaborativeReboot": {
                "ScheduledRequests": {
                    "SystemUpdate": 2u64,
                    "NetstackMigration": 1u64,
                }
            }
        });

        // Cancel the `SystemUpdate` requests, and verify the inspect data.
        std::mem::drop(cancellation_signals);
        state.cancellation_sender.close_channel();
        cancellations_worker.run().await;
        assert_data_tree!(inspector, root: {
            "CollaborativeReboot": {
                "ScheduledRequests": {
                    "SystemUpdate": 0u64,
                    "NetstackMigration": 1u64,
                }
            }
        });
    }
}
