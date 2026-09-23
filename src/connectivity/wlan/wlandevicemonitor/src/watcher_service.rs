// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fidl_fuchsia_wlan_device_service::{self as fidl_svc, DeviceWatcherControlHandle};
use fuchsia_async::Task;
use fuchsia_sync::Mutex;
use futures::channel::mpsc::UnboundedReceiver;
use futures::prelude::*;
use futures::try_join;
use log::error;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use crate::watchable_map::{MapEvent, WatchableMap};

// In reality, P and I are always PhyDevice and IfaceDevice, respectively.
// They are generic solely for the purpose of mocking for tests.
pub fn serve_watchers<P, I>(
    phys: Arc<WatchableMap<u16, P>>,
    ifaces: Arc<WatchableMap<u16, I>>,
    phy_events: UnboundedReceiver<MapEvent<u16, P>>,
    iface_events: UnboundedReceiver<MapEvent<u16, I>>,
) -> (WatcherService<P, I>, impl Future<Output = Result<Infallible, anyhow::Error>>) {
    let inner =
        Arc::new(Mutex::new(Inner { watchers: HashMap::new(), next_watcher_id: 0, phys, ifaces }));
    let s = WatcherService { inner: Arc::clone(&inner) };

    let fut = async move {
        let phy_fut = notify_phy_watchers(phy_events, &inner);
        let iface_fut = notify_iface_watchers(iface_events, &inner);
        try_join!(phy_fut, iface_fut).map(|x: (Infallible, Infallible)| x.0)
    };
    (s, fut)
}

pub struct WatcherService<P, I> {
    inner: Arc<Mutex<Inner<P, I>>>,
}

// Manual clone impl since #derive uses incorrect trait bounds
impl<P, I> Clone for WatcherService<P, I> {
    fn clone(&self) -> Self {
        WatcherService { inner: self.inner.clone() }
    }
}

// P and I must have static lifetime because add_watcher spawns a task for each watcher
// that may drop a device when its watcher closes the channel. The task itself exists
// independent of the lifetime of the values in each WatchableMap.
impl<P: 'static, I: 'static> WatcherService<P, I> {
    // This function will panic if not called in the context of a
    // running fuchsia_async executor.
    pub fn add_watcher(
        &self,
        endpoint: ServerEnd<fidl_svc::DeviceWatcherMarker>,
    ) -> Result<(), anyhow::Error> {
        let stream = endpoint.into_stream();
        let handle = stream.control_handle();
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        // Apply backpressure if the watcher limit is reached to avoid resource exhaustion.
        if inner.watchers.len() >= WATCHER_LIMIT {
            return Err(format_err!("too many watchers"));
        }

        let watcher_id = inner.next_watcher_id;
        inner.next_watcher_id += 1;

        // Spawn a task that removes watchers from device maps when their FIDL channels
        // close. Otherwise, watchers will not be removed until a notification fails
        // to be sent.
        Task::local({
            // Wait for the other side to close the channel (or an error to occur)
            // and remove the watcher from the maps

            let inner = self.inner.clone();
            async move {
                stream.map(|_| ()).collect::<()>().await;
                inner.lock().watchers.remove(&watcher_id);
            }
        })
        .detach();
        inner.watchers.insert(
            watcher_id,
            Watcher { handle, sent_phy_snapshot: false, sent_iface_snapshot: false },
        );
        inner.phys.request_snapshot();
        inner.ifaces.request_snapshot();
        Ok(())
    }
}

// Arbitrarily high limit to prevent unbounded number of watchers.
const WATCHER_LIMIT: usize = 10000;
struct Inner<P, I> {
    watchers: HashMap<u64, Watcher>,
    next_watcher_id: u64,
    phys: Arc<WatchableMap<u16, P>>,
    ifaces: Arc<WatchableMap<u16, I>>,
}

struct Watcher {
    handle: DeviceWatcherControlHandle,
    sent_phy_snapshot: bool,
    sent_iface_snapshot: bool,
}

impl<P, I> Inner<P, I> {
    fn notify_watchers<F, G>(&mut self, sent_snapshot: F, send_event: G)
    where
        F: Fn(&Watcher) -> bool,
        G: Fn(&DeviceWatcherControlHandle) -> Result<(), fidl::Error>,
    {
        self.watchers.retain(|_, w| {
            if sent_snapshot(w) {
                let r = send_event(&w.handle);
                handle_send_result(&w.handle, r)
            } else {
                true
            }
        })
    }

    fn send_snapshot<F, G, T>(
        &mut self,
        sent_snapshot: F,
        send_on_add: G,
        snapshot: Arc<HashMap<u16, T>>,
    ) where
        F: Fn(&mut Watcher) -> &mut bool,
        G: Fn(&DeviceWatcherControlHandle, u16) -> Result<(), fidl::Error>,
    {
        self.watchers.retain(|_, w| {
            if !*sent_snapshot(w) {
                for key in snapshot.keys() {
                    let r = send_on_add(&w.handle, *key);
                    if !handle_send_result(&w.handle, r) {
                        return false;
                    }
                }
                *sent_snapshot(w) = true;
            }
            true
        })
    }
}

fn handle_send_result(handle: &DeviceWatcherControlHandle, r: Result<(), fidl::Error>) -> bool {
    if let Err(e) = r.as_ref() {
        error!("Error sending event to watcher: {}", e);
        handle.shutdown();
    }
    r.is_ok()
}

async fn notify_phy_watchers<P, I>(
    mut events: UnboundedReceiver<MapEvent<u16, P>>,
    inner: &Mutex<Inner<P, I>>,
) -> Result<Infallible, anyhow::Error> {
    while let Some(e) = events.next().await {
        match e {
            MapEvent::KeyInserted(id) => {
                inner.lock().notify_watchers(|w| w.sent_phy_snapshot, |h| h.send_on_phy_added(id))
            }
            MapEvent::KeyRemoved(id) => {
                inner.lock().notify_watchers(|w| w.sent_phy_snapshot, |h| h.send_on_phy_removed(id))
            }
            MapEvent::Snapshot(s) => inner.lock().send_snapshot(
                |w| &mut w.sent_phy_snapshot,
                |h, id| h.send_on_phy_added(id),
                s,
            ),
        }
    }
    Err(format_err!("stream of events from the phy device map has ended unexpectedly"))
}

async fn notify_iface_watchers<P, I>(
    mut events: UnboundedReceiver<MapEvent<u16, I>>,
    inner: &Mutex<Inner<P, I>>,
) -> Result<Infallible, anyhow::Error> {
    while let Some(e) = events.next().await {
        match e {
            MapEvent::KeyInserted(id) => inner
                .lock()
                .notify_watchers(|w| w.sent_iface_snapshot, |h| h.send_on_iface_added(id)),
            MapEvent::KeyRemoved(id) => inner
                .lock()
                .notify_watchers(|w| w.sent_iface_snapshot, |h| h.send_on_iface_removed(id)),
            MapEvent::Snapshot(s) => inner.lock().send_snapshot(
                |w| &mut w.sent_iface_snapshot,
                |h, id| h.send_on_iface_added(id),
                s,
            ),
        }
    }
    Err(format_err!("stream of events from the iface device map has ended unexpectedly"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_wlan_device_service::DeviceWatcherEvent;
    use fuchsia_async::TestExecutor;
    use futures::task::Poll;
    use rand::RngExt as _;
    use std::mem;
    use std::pin::pin;

    #[fuchsia::test(allow_stalls = false)]
    async fn reaper_destroys_watcher_after_losing_connection() {
        let (helper, future) = setup();
        let mut future = pin!(future);
        assert_eq!(0, helper.service.inner.lock().watchers.len());
        let (client_end, server_end) = fidl::endpoints::create_endpoints();

        // Add a watcher and check that it was added to the map
        helper.service.add_watcher(server_end).expect("add_watcher failed");
        assert_eq!(1, helper.service.inner.lock().watchers.len());

        // Run the reaper and make sure the watcher is still there
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("future returned an error (1): {e:?}");
        }
        assert_eq!(1, helper.service.inner.lock().watchers.len());

        // Drop the client end of the channel and run the reaper again
        mem::drop(client_end);
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("future returned an error (1): {e:?}");
        }
        assert_eq!(0, helper.service.inner.lock().watchers.len());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn reaper_can_handle_many_active_watchers() {
        let (helper, future) = setup();
        let mut future = pin!(future);
        let mut rng = rand::rng();
        assert_eq!(0, helper.service.inner.lock().watchers.len());

        // Add lots of watchers and randomly remove some at uneven intervals.
        let mut active_clients = Vec::with_capacity(2000);
        for _ in 0..2000 {
            let (client_end, server_end) = fidl::endpoints::create_endpoints();
            helper.service.add_watcher(server_end).expect("add_watcher failed");
            active_clients.push(client_end);
            assert_eq!(active_clients.len(), helper.service.inner.lock().watchers.len());

            // Run the reaper and make sure the watcher is still there
            if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
                panic!("future returned an error (1): {e:?}");
            }
            assert_eq!(active_clients.len(), helper.service.inner.lock().watchers.len());

            // Remove some of the watchers every ~100 iterations
            if rng.random_bool(0.01) {
                active_clients.retain(|_| rng.random_bool(0.9));

                if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
                    panic!("future returned an error (1): {e:?}");
                }
                assert_eq!(active_clients.len(), helper.service.inner.lock().watchers.len());
            }
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn active_watchers_limited_at_watcher_limit() {
        let (helper, future) = setup();
        let mut future = pin!(future);

        // Add lots of watchers and randomly remove some at uneven intervals.
        let mut active_clients = Vec::with_capacity(2000);
        for _ in 0..WATCHER_LIMIT {
            let (client_end, server_end) = fidl::endpoints::create_endpoints();
            helper.service.add_watcher(server_end).expect("add_watcher failed");
            active_clients.push(client_end);
            if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
                panic!("future returned an error (1): {e:?}");
            }
        }

        let (_, server_end) = fidl::endpoints::create_endpoints();

        // Add a watcher and check that it was added to the map
        helper.service.add_watcher(server_end).expect_err("added a watcher beyond WATCHER_LIMIT");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_receives_one_to_one_in_order_phy_events() {
        let (helper, future) = setup();
        let mut future = pin!(future);
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end).expect("add_watcher failed");

        helper.phys.insert(20, 2000);
        helper.phys.insert(30, 3000);
        helper.phys.remove(&20);

        // Run the server future to propagate the events to FIDL clients
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }

        let events = consume_events_until_stalled(proxy.take_event_stream()).await;
        assert_eq!(3, events.len());
        // Sadly, generated Event struct doesn't implement PartialEq
        match &events[0] {
            &DeviceWatcherEvent::OnPhyAdded { phy_id: 20 } => {}
            other => panic!("Expected OnPhyAdded(20), got {other:?}"),
        }
        match &events[1] {
            &DeviceWatcherEvent::OnPhyAdded { phy_id: 30 } => {}
            other => panic!("Expected OnPhyAdded(30), got {other:?}"),
        }
        match &events[2] {
            &DeviceWatcherEvent::OnPhyRemoved { phy_id: 20 } => {}
            other => panic!("Expected OnPhyRemoved(20), got {other:?}"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_receives_one_to_one_in_order_iface_events() {
        let (helper, future) = setup();
        let mut future = pin!(future);
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end).expect("add_watcher failed");

        helper.ifaces.insert(50, 5000);
        helper.ifaces.remove(&50);

        // Run the server future to propagate the events to FIDL clients
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }

        let events = consume_events_until_stalled(proxy.take_event_stream()).await;
        assert_eq!(2, events.len());
        match &events[0] {
            &DeviceWatcherEvent::OnIfaceAdded { iface_id: 50 } => {}
            other => panic!("Expected OnIfaceAdded(50), got {other:?}"),
        }
        match &events[1] {
            &DeviceWatcherEvent::OnIfaceRemoved { iface_id: 50 } => {}
            other => panic!("Expected OnIfaceRemoved(50), got {other:?}"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn initial_snapshot_events_only_contain_present_phys() {
        let (helper, future) = setup();
        let mut future = pin!(future);

        // Add and remove phys before we the watcher is added
        helper.phys.insert(20, 2000);
        helper.phys.insert(30, 3000);
        helper.phys.remove(&20);

        // Now add the watcher and pump the events
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end).expect("add_watcher failed");
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }

        // The watcher should only see phy #30 being "added"
        let events = consume_events_until_stalled(proxy.take_event_stream()).await;
        assert_eq!(1, events.len());
        match &events[0] {
            &DeviceWatcherEvent::OnPhyAdded { phy_id: 30 } => {}
            other => panic!("Expected OnPhyAdded(30), got {other:?}"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn initial_snapshot_events_only_contain_present_ifaces() {
        let (helper, future) = setup();
        let mut future = pin!(future);

        // Add and remove ifaces before we the watcher is added
        helper.ifaces.insert(20, 2000);
        helper.ifaces.insert(30, 3000);
        helper.ifaces.remove(&20);

        // Now add the watcher and pump the events
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end).expect("add_watcher failed");
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }

        // The watcher should only see iface #30 being "added"
        let events = consume_events_until_stalled(proxy.take_event_stream()).await;
        assert_eq!(1, events.len());
        match &events[0] {
            &DeviceWatcherEvent::OnIfaceAdded { iface_id: 30 } => {}
            other => panic!("Expected OnIfaceAdded(30), got {other:?}"),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn initial_updates_for_multiple_watchers_are_identical() {
        let (helper, future) = setup();
        let mut future = pin!(future);

        helper.phys.insert(10, 1000);
        helper.ifaces.insert(20, 2000);
        helper.ifaces.insert(30, 3000);

        // Add watchers
        let (proxy_one, server_end_one) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end_one).expect("add_watcher failed (1)");
        let (proxy_two, server_end_two) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end_two).expect("add_watcher failed (2)");
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }
        let events_one = consume_events_until_stalled(proxy_one.take_event_stream()).await;
        assert_eq!(3, events_one.len());
        let events_two = consume_events_until_stalled(proxy_two.take_event_stream()).await;
        expect_lists_are_identical(events_one, events_two);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn adding_watchers_does_not_retrigger_snapshots() {
        let (helper, future) = setup();
        let mut future = pin!(future);

        helper.phys.insert(10, 1000);
        helper.ifaces.insert(20, 2000);

        // Add first watcher
        let (proxy_one, server_end_one) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end_one).expect("add_watcher failed (1)");
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }
        // Consume events from snapshot upon adding the first watcher
        let events_one = consume_events_until_stalled(proxy_one.take_event_stream()).await;
        assert_eq!(2, events_one.len());

        // Add second watcher
        let (proxy_two, server_end_two) = fidl::endpoints::create_proxy();
        helper.service.add_watcher(server_end_two).expect("add_watcher failed (2)");
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("server future returned an error: {e:?}");
        }
        // Check that the first watcher did not retrigger a snapshot.
        assert_eq!(consume_events_until_stalled(proxy_one.take_event_stream()).await.len(), 0);
        // Consume events from snapshot upon adding the second watcher
        let events_two = consume_events_until_stalled(proxy_two.take_event_stream()).await;
        expect_lists_are_identical(events_one, events_two);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn error_sending_an_update_destroys_the_watcher() {
        let (helper, future) = setup();
        let mut future = pin!(future);

        let (client_chan, server_chan) = zx::Channel::create();
        // Make a channel without a WRITE permission to make sure sending an event fails
        let server_handle: zx::NullableHandle = server_chan.into();
        let reduced_chan: zx::Channel =
            server_handle.replace_handle(zx::Rights::READ | zx::Rights::WAIT).unwrap().into();

        helper.service.add_watcher(ServerEnd::new(reduced_chan)).expect("add_watcher failed");
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("future returned an error (1): {e:?}");
        }
        assert_eq!(1, helper.service.inner.lock().watchers.len());

        // Not add a phy to trigger an event
        helper.phys.insert(20, 2000);

        // The watcher should be now removed since sending the event fails
        if let Poll::Ready(Err(e)) = TestExecutor::poll_until_stalled(&mut future).await {
            panic!("future returned an error (1): {e:?}");
        }
        assert_eq!(0, helper.service.inner.lock().watchers.len());

        // Make sure the client endpoint is only dropped at the end, so that the watcher
        // is not removed by the reaper thread
        mem::drop(client_chan);
    }

    struct Helper {
        phys: Arc<WatchableMap<u16, i32>>,
        ifaces: Arc<WatchableMap<u16, i32>>,
        service: WatcherService<i32, i32>,
    }

    fn setup() -> (Helper, impl Future<Output = Result<Infallible, anyhow::Error>>) {
        let (phys, phy_events) = WatchableMap::new();
        let (ifaces, iface_events) = WatchableMap::new();
        let phys = Arc::new(phys);
        let ifaces = Arc::new(ifaces);
        let (service, future) =
            serve_watchers(phys.clone(), ifaces.clone(), phy_events, iface_events);
        let helper = Helper { phys, ifaces, service };
        (helper, future)
    }

    async fn consume_events_until_stalled(
        mut stream: fidl_svc::DeviceWatcherEventStream,
    ) -> Vec<DeviceWatcherEvent> {
        let mut events = vec![];
        loop {
            match TestExecutor::poll_until_stalled(&mut stream.try_next()).await {
                Poll::Ready(Err(e)) => panic!("event stream future returned an error: {e:?}"),
                Poll::Pending | Poll::Ready(Ok(None)) => break events,
                Poll::Ready(Ok(Some(event))) => events.push(event),
            }
        }
    }

    // Simple comparator for DeviceWatcherEvent since it does not implement PartialEq.
    fn events_are_identical(a: &DeviceWatcherEvent, b: &DeviceWatcherEvent) -> bool {
        match (a, b) {
            (
                DeviceWatcherEvent::OnPhyAdded { phy_id: x },
                DeviceWatcherEvent::OnPhyAdded { phy_id: y },
            )
            | (
                DeviceWatcherEvent::OnPhyRemoved { phy_id: x },
                DeviceWatcherEvent::OnPhyRemoved { phy_id: y },
            )
            | (
                DeviceWatcherEvent::OnIfaceAdded { iface_id: x },
                DeviceWatcherEvent::OnIfaceAdded { iface_id: y },
            )
            | (
                DeviceWatcherEvent::OnIfaceRemoved { iface_id: x },
                DeviceWatcherEvent::OnIfaceRemoved { iface_id: y },
            ) => x == y,
            _ => false,
        }
    }

    fn expect_lists_are_identical(a: Vec<DeviceWatcherEvent>, b: Vec<DeviceWatcherEvent>) {
        if a.len() != b.len() {
            panic!("event lists have different lengths: {a:?} != {b:?}");
        }
        a.iter().zip(b.iter()).for_each(|(x, y)| {
            if !events_are_identical(x, y) {
                panic!("event lists are not identical: {a:?} != {b:?}");
            }
        });
    }
}
