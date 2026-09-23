// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use driver_manager_bind::BindManagerHandle;
use driver_manager_driver_host::DriverHost;
use driver_manager_node::Node;
use fuchsia_async as fasync;
use fuchsia_async::DurationExt;
use futures::FutureExt;
use futures::channel::oneshot;
use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

const BOOTUP_TIMEOUT_DURATION: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(2);
const LAST_UPDATED_TIMEOUT_DURATION: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(10);
const MAX_TIMEOUT_DURATION: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(60);

struct StartRequest {
    driver_url: String,
    node: Weak<Node>,
}

struct BootupTrackerInner {
    outstanding_start_requests: HashMap<String, StartRequest>,
    bootup_done: bool,
    waiters: Vec<oneshot::Sender<()>>,
    last_update_timestamp: zx::MonotonicInstant,
    bootup_timeout_task: Option<fasync::Task<()>>,
    bootup_timeout_sender: Option<oneshot::Sender<()>>,
    current_timeout: zx::MonotonicDuration,
}

pub struct BootupTracker {
    bind_manager: BindManagerHandle,
    inner: RefCell<BootupTrackerInner>,
    weak_self: Weak<Self>,
}

impl BootupTracker {
    pub fn new(bind_manager: BindManagerHandle) -> Rc<Self> {
        Rc::new_cyclic(|weak_self| Self {
            bind_manager,
            inner: RefCell::new(BootupTrackerInner {
                outstanding_start_requests: HashMap::new(),
                bootup_done: false,
                waiters: Vec::new(),
                last_update_timestamp: zx::MonotonicInstant::get(),
                bootup_timeout_task: None,
                bootup_timeout_sender: None,
                current_timeout: BOOTUP_TIMEOUT_DURATION,
            }),
            weak_self: weak_self.clone(),
        })
    }

    pub fn start(&self) {
        self.update_tracker_and_reset_timer();
    }

    pub async fn wait_for_bootup(&self) {
        if self.inner.borrow().bootup_done {
            return;
        }
        let (tx, rx) = oneshot::channel();
        self.inner.borrow_mut().waiters.push(tx);
        let _ = rx.await;
    }

    pub fn notify_new_start_request(
        &self,
        node_moniker: String,
        driver_url: String,
        node: Weak<Node>,
    ) {
        if self.inner.borrow().outstanding_start_requests.contains_key(&node_moniker) {
            warn!("Bootup tracker received conflicting start requests for node {}", node_moniker);
        }
        self.inner
            .borrow_mut()
            .outstanding_start_requests
            .insert(node_moniker, StartRequest { driver_url, node });
        self.update_tracker_and_reset_timer();
    }

    pub fn notify_start_complete(&self, node_moniker: &str) {
        if self.inner.borrow_mut().outstanding_start_requests.remove(node_moniker).is_none() {
            info!("Bootup tracker notified for an unknown start request for {}", node_moniker);
        }
        self.update_tracker_and_reset_timer();
    }

    pub fn notify_binding_changed(&self) {
        self.update_tracker_and_reset_timer();
    }

    fn check_bootup_done(&self) {
        if self.is_update_deadline_exceeded()
            && (!self.inner.borrow().outstanding_start_requests.is_empty()
                || self.bind_manager.has_ongoing_bind())
        {
            // This log message is used by tefmocheck to detect driver start/bind hangs.
            // LINT.IfChange
            warn!("Deadline exceeded in the bootup tracker with:");
            // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go)
            warn!(
                "    {} unfinished start requests:",
                self.inner.borrow().outstanding_start_requests.len()
            );
            let mut driver_hosts: Vec<Rc<dyn DriverHost>> = Vec::new();
            for (moniker, request) in self.inner.borrow().outstanding_start_requests.iter() {
                warn!("         - {} - {}", moniker, request.driver_url);
                if let Some(host) = request.node.upgrade().and_then(|node| node.driver_host())
                    && !driver_hosts.iter().any(|h| Rc::ptr_eq(h, &host))
                {
                    driver_hosts.push(host);
                }
            }
            for host in driver_hosts {
                host.trigger_stack_trace();
            }
            if self.bind_manager.has_ongoing_bind() {
                warn!("    a hanging bind process in the bind manager");
            }

            let mut inner = self.inner.borrow_mut();
            inner.current_timeout = inner.current_timeout * 2;
            if inner.current_timeout > MAX_TIMEOUT_DURATION {
                inner.current_timeout = MAX_TIMEOUT_DURATION;
            }
        }

        if !self.inner.borrow().outstanding_start_requests.is_empty()
            || self.bind_manager.has_ongoing_bind()
        {
            self.reset_bootup_timer();
            return;
        }

        info!("Bootup completed.");

        for sender in std::mem::take(&mut self.inner.borrow_mut().waiters) {
            let _ = sender.send(());
        }
        self.inner.borrow_mut().bootup_done = true;
        if let Some(sender) = self.inner.borrow_mut().bootup_timeout_sender.take() {
            let _ = sender.send(());
        }
        self.inner.borrow_mut().bootup_timeout_task = None;
    }

    fn update_tracker_and_reset_timer(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.last_update_timestamp = zx::MonotonicInstant::get();
        inner.current_timeout = BOOTUP_TIMEOUT_DURATION;
        drop(inner);
        self.reset_bootup_timer();
    }

    fn on_bootup_timeout(weak_self: Weak<Self>) {
        if let Some(arc_self) = weak_self.upgrade() {
            arc_self.check_bootup_done();
        }
    }

    fn is_update_deadline_exceeded(&self) -> bool {
        zx::MonotonicInstant::get() - self.inner.borrow().last_update_timestamp
            >= LAST_UPDATED_TIMEOUT_DURATION
    }

    fn reset_bootup_timer(&self) {
        if self.inner.borrow().bootup_done {
            return;
        }

        if let Some(sender) = self.inner.borrow_mut().bootup_timeout_sender.take() {
            let _ = sender.send(());
        }

        let (sender, receiver) = oneshot::channel();
        self.inner.borrow_mut().bootup_timeout_sender = Some(sender);

        let weak_self = self.weak_self.clone();
        let timeout = self.inner.borrow().current_timeout;
        let timer = fasync::Timer::new(timeout.after_now());
        let task = fasync::Task::local(async move {
            futures::select! {
                _ = timer.fuse() => {
                    BootupTracker::on_bootup_timeout(weak_self);
                },
                _ = receiver.fuse() => {
                    // Task was cancelled.
                },
            }
        });
        self.inner.borrow_mut().bootup_timeout_task = Some(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MockDriverHost, MockNodeManager};
    use driver_manager_bind::{BindManagerBridge, BindSpecResult};
    use driver_manager_node::{Node, NodeManager};
    use std::sync::atomic::Ordering;

    use fidl_fuchsia_driver_framework as fdf;
    use fidl_fuchsia_driver_index as fdi;

    struct MockBindManagerBridge;
    #[async_trait::async_trait(?Send)]
    impl BindManagerBridge for MockBindManagerBridge {
        fn box_clone(&self) -> Box<dyn BindManagerBridge> {
            Box::new(MockBindManagerBridge)
        }
        fn on_binding_state_changed(&self) {}
        async fn request_match_from_driver_index(
            &self,
            _args: fdi::MatchDriverArgs,
        ) -> fidl::Result<fdi::MatchDriverResult> {
            Ok(fdi::MatchDriverResult::Driver(fdf::DriverInfo::default()))
        }
        async fn start_driver(
            &self,
            _node: &Rc<Node>,
            _driver_info: fdf::DriverInfo,
        ) -> Result<String, zx::Status> {
            Ok("".to_string())
        }
        fn bind_to_parent_spec(
            &self,
            _parents: &[fdf::CompositeParent],
            _node: Weak<Node>,
            _enable_multibind: bool,
        ) -> Result<BindSpecResult, zx::Status> {
            Err(zx::Status::NOT_SUPPORTED)
        }
    }

    #[fuchsia::test]
    async fn test_exponential_backoff_and_stack_trace() {
        let bind_manager = BindManagerHandle::new(Box::new(MockBindManagerBridge));
        let tracker = BootupTracker::new(bind_manager);

        let node_manager = Box::new(MockNodeManager);
        let node = Node::new("test_node", Weak::new(), node_manager);
        let host = Rc::new(MockDriverHost::new());
        node.set_host(host.clone());

        tracker.notify_new_start_request(
            "node_1".to_string(),
            "url_1".to_string(),
            Rc::downgrade(&node),
        );

        // Initial timeout should be 2s.
        assert_eq!(tracker.inner.borrow().current_timeout, BOOTUP_TIMEOUT_DURATION);

        // Manually trigger timeout. Deadline won't be exceeded yet.
        tracker.check_bootup_done();
        assert_eq!(tracker.inner.borrow().current_timeout, BOOTUP_TIMEOUT_DURATION);
        assert_eq!(host.stack_trace_count.load(Ordering::SeqCst), 0);

        // Force deadline exceedance.
        tracker.inner.borrow_mut().last_update_timestamp =
            zx::MonotonicInstant::get() - LAST_UPDATED_TIMEOUT_DURATION;

        tracker.check_bootup_done();
        // Timeout should double.
        assert_eq!(tracker.inner.borrow().current_timeout, BOOTUP_TIMEOUT_DURATION * 2);
        // Stack trace should be triggered.
        assert_eq!(host.stack_trace_count.load(Ordering::SeqCst), 1);

        // Trigger again.
        tracker.inner.borrow_mut().last_update_timestamp =
            zx::MonotonicInstant::get() - LAST_UPDATED_TIMEOUT_DURATION;
        tracker.check_bootup_done();
        assert_eq!(tracker.inner.borrow().current_timeout, BOOTUP_TIMEOUT_DURATION * 4);
        assert_eq!(host.stack_trace_count.load(Ordering::SeqCst), 2);

        // New request should reset timeout.
        tracker.notify_new_start_request("node_2".to_string(), "url_2".to_string(), Weak::new());
        assert_eq!(tracker.inner.borrow().current_timeout, BOOTUP_TIMEOUT_DURATION);
    }

    #[fuchsia::test]
    async fn test_deduplicate_driver_hosts() {
        let bind_manager = BindManagerHandle::new(Box::new(MockBindManagerBridge));
        let tracker = BootupTracker::new(bind_manager);

        let node_manager = Box::new(MockNodeManager);
        let node1 = Node::new("node1", Weak::new(), node_manager.clone_box());
        let node2 = Node::new("node2", Weak::new(), node_manager.clone_box());

        let host = Rc::new(MockDriverHost::new());
        node1.set_host(host.clone());
        node2.set_host(host.clone());

        tracker.notify_new_start_request(
            "node_1".to_string(),
            "url_1".to_string(),
            Rc::downgrade(&node1),
        );
        tracker.notify_new_start_request(
            "node_2".to_string(),
            "url_2".to_string(),
            Rc::downgrade(&node2),
        );

        tracker.inner.borrow_mut().last_update_timestamp =
            zx::MonotonicInstant::get() - LAST_UPDATED_TIMEOUT_DURATION;
        tracker.check_bootup_done();

        // Stack trace should only be triggered once for the shared host.
        assert_eq!(host.stack_trace_count.load(Ordering::SeqCst), 1);
    }
}
