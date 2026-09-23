// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, bail};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_wlan_device_service as fidl_svc;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::stream::FuturesUnordered;
use futures::{Future, StreamExt, select};
use log::warn;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use crate::device::PhyEvent;

pub fn serve_phy_events(
    phy_event_stream: mpsc::Receiver<(u16, PhyEvent)>,
) -> (PhyEventService, impl Future<Output = Result<Infallible, Error>>) {
    let (watcher_sender, watcher_stream) = mpsc::unbounded();
    let inner = Arc::new(Mutex::new(Inner {
        watchers: HashMap::new(),
        next_watcher_id: 0,
        watcher_sender,
    }));
    let service = PhyEventService { inner: Arc::clone(&inner) };

    let fut = notify_phy_event_watchers(phy_event_stream, watcher_stream, inner);

    (service, fut)
}

async fn notify_phy_event_watchers(
    mut phy_event_stream: mpsc::Receiver<(u16, PhyEvent)>,
    mut watcher_stream: mpsc::UnboundedReceiver<ServerEnd<fidl_svc::PhyEventWatcherMarker>>,
    inner: Arc<Mutex<Inner>>,
) -> Result<Infallible, Error> {
    let mut active_watchers = FuturesUnordered::new();
    loop {
        #[rustfmt::skip]
        select! {
            phy_event = phy_event_stream.next() => match phy_event {
                Some((phy_id, event)) =>  {
                    match event {
                        PhyEvent::OnCriticalError { reason_code } => {
                            let locked = inner.lock();
                            if locked.watchers.is_empty() {
                                warn!(
                                    "Received critical error {:?} on phy {} while no watchers are present",
                                    reason_code, phy_id
                                );
                            }
                            for (watcher_id, watcher) in locked.watchers.iter() {
                                if let Err(e) =
                                    watcher.send_on_critical_error(phy_id, reason_code)
                                {
                                    warn!(
                                        "Failed to send on critical error to watcher {}: {}",
                                        watcher_id, e
                                    );
                                }
                            }
                        }
                        PhyEvent::OnCountryCodeChange { .. } => {
                            warn!("Received country code change indication");
                        }
                    }
                }
                None => {
                    bail!("phy event stream ended unexpectedly");
                }
            },
            watcher = watcher_stream.next() => match watcher {
                Some(watcher) => {
                    let (stream, handle) = watcher.into_stream_and_control_handle();
                    let watcher_id = {
                        let mut inner = inner.lock();
                        let watcher_id = inner.next_watcher_id;
                        inner.next_watcher_id += 1;

                        inner.watchers.insert(watcher_id, handle);
                        watcher_id
                    };

                    let inner_clone = inner.clone();
                    let removal_fut = async move {
                        stream.map(|_| ()).collect::<()>().await;
                        inner_clone.lock().watchers.remove(&watcher_id);
                    };
                    active_watchers.push(removal_fut);
                },
                None => {
                    bail!("watcher stream ended unexpectedly");
                }
            },
            () = active_watchers.select_next_some() => {},
        }
    }
}

pub struct PhyEventService {
    inner: Arc<Mutex<Inner>>,
}

impl PhyEventService {
    pub fn add_watcher(&self, endpoint: ServerEnd<fidl_svc::PhyEventWatcherMarker>) {
        if let Err(e) = self.inner.lock().watcher_sender.unbounded_send(endpoint) {
            warn!("Failed to add phy watcher: {}", e);
        }
    }
}

struct Inner {
    watchers: HashMap<u64, fidl_svc::PhyEventWatcherControlHandle>,
    next_watcher_id: u64,
    watcher_sender: mpsc::UnboundedSender<ServerEnd<fidl_svc::PhyEventWatcherMarker>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fuchsia_async::TestExecutor;
    use futures::task::Poll;
    use std::pin::pin;

    #[test]
    fn event_service_forwards_phy_reset_to_watchers() {
        let mut exec = TestExecutor::new();

        let (mut phy_event_sink, phy_event_stream) = mpsc::channel(5);
        let (service, fut) = serve_phy_events(phy_event_stream);
        let mut fut = pin!(fut);

        let (svc_proxy1, svc_server1) = create_proxy::<fidl_svc::PhyEventWatcherMarker>();
        let mut svc_events1 = svc_proxy1.take_event_stream();
        service.add_watcher(svc_server1);

        let (svc_proxy2, svc_server2) = create_proxy::<fidl_svc::PhyEventWatcherMarker>();
        let mut svc_events2 = svc_proxy2.take_event_stream();
        service.add_watcher(svc_server2);

        let mut next_event1 = svc_events1.next();
        let mut next_event2 = svc_events2.next();
        assert_matches!(exec.run_until_stalled(&mut next_event1), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut next_event2), Poll::Pending);

        phy_event_sink
            .try_send((
                123,
                crate::device::PhyEvent::OnCriticalError {
                    reason_code: fidl_internal::CriticalErrorReason::FwCrash,
                },
            ))
            .expect("Failed to send event");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let event1 = assert_matches!(exec.run_until_stalled(&mut next_event1), Poll::Ready(Some(Ok(event))) => event);
        assert_matches!(
            event1,
            fidl_svc::PhyEventWatcherEvent::OnCriticalError {
                phy_id: 123,
                reason_code: fidl_internal::CriticalErrorReason::FwCrash,
            }
        );
        let event2 = assert_matches!(exec.run_until_stalled(&mut next_event2), Poll::Ready(Some(Ok(event))) => event);
        assert_matches!(
            event2,
            fidl_svc::PhyEventWatcherEvent::OnCriticalError {
                phy_id: 123,
                reason_code: fidl_internal::CriticalErrorReason::FwCrash,
            }
        );
    }

    #[test]
    fn dropped_watcher_unregisters() {
        let mut exec = TestExecutor::new();

        let (_phy_event_sink, phy_event_stream) = mpsc::channel(5);
        let (service, fut) = serve_phy_events(phy_event_stream);
        let mut fut = pin!(fut);

        let (svc_proxy, svc_server) = create_proxy::<fidl_svc::PhyEventWatcherMarker>();
        service.add_watcher(svc_server);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_eq!(service.inner.lock().watchers.len(), 1);

        std::mem::drop(svc_proxy);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert!(service.inner.lock().watchers.is_empty());
    }

    #[test]
    fn dropped_phy_event_stream_ends_service() {
        let mut exec = TestExecutor::new();

        let (phy_event_sink, phy_event_stream) = mpsc::channel(5);
        let (_service, fut) = serve_phy_events(phy_event_stream);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        std::mem::drop(phy_event_sink);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }
}
