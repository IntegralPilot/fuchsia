// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bind_node_set::BindNodeSet;
use async_trait::async_trait;
use driver_manager_node::Node;
use driver_manager_types::{BindResult, BindResultTracker, NodeType};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_index as fdi;
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use log::{error, warn};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub type NodeBindingInfoResultCompleter = oneshot::Sender<Vec<fdd::NodeBindingInfo>>;

pub struct CompositeNodeAndDriver {
    pub driver: fdf::CompositeDriverInfo,
    pub node: Weak<Node>,
}

pub struct BindSpecResult {
    pub bound_composite_parents: Vec<fdf::CompositeParent>,
    pub completed_node_and_drivers: Vec<CompositeNodeAndDriver>,
}

#[derive(Debug)]
pub struct BindRequest {
    node_moniker: String,
    node: Weak<Node>,
    driver_url_suffix: String,
    tracker: Rc<RefCell<BindResultTracker>>,
    composite_only: bool,
}

#[async_trait(?Send)]
pub trait BindManagerBridge {
    fn box_clone(&self) -> Box<dyn BindManagerBridge>;
    fn on_binding_state_changed(&self);
    async fn request_match_from_driver_index(
        &self,
        args: fdi::MatchDriverArgs,
    ) -> fidl::Result<fdi::MatchDriverResult>;
    async fn start_driver(
        &self,
        node: &Rc<Node>,
        driver_info: fdf::DriverInfo,
    ) -> Result<String, zx::Status>;
    fn bind_to_parent_spec(
        &self,
        parents: &[fdf::CompositeParent],
        node: Weak<Node>,
        enable_multibind: bool,
    ) -> Result<BindSpecResult, zx::Status>;
}

pub struct BindManager {
    bridge: Box<dyn BindManagerBridge>,
    bind_node_set: RefCell<BindNodeSet>,
    pending_bind_requests: RefCell<Vec<BindRequest>>,
    pending_orphan_rebind_completers: RefCell<Vec<NodeBindingInfoResultCompleter>>,
    weak_self: Weak<Self>,
    scope: fasync::Scope,
}

// This handle is what other parts of the driver manager will use to interact with the
// BindManager. It's a separate struct so that we can wrap the BindManager in a Mutex
// and control access.
#[derive(Clone)]
pub struct BindManagerHandle(Rc<BindManager>);

impl BindManager {
    pub fn new(bridge: Box<dyn BindManagerBridge>) -> Rc<Self> {
        Rc::new_cyclic(|weak_self| {
            let mut bind_node_set = BindNodeSet::new();
            let (sender, mut receiver) = mpsc::unbounded();
            bind_node_set.set_on_bind_state_changed(sender);

            let scope = fasync::Scope::new_with_name("bind_manager");
            let bridge_clone = bridge.box_clone();
            scope.spawn_local(async move {
                while let Some(()) = receiver.next().await {
                    bridge_clone.on_binding_state_changed();
                }
            });

            Self {
                bridge,
                bind_node_set: RefCell::new(bind_node_set),
                pending_bind_requests: RefCell::new(Vec::new()),
                pending_orphan_rebind_completers: RefCell::new(Vec::new()),
                weak_self: weak_self.clone(),
                scope,
            }
        })
    }

    pub fn bind(
        &self,
        node: &Rc<Node>,
        driver_url_suffix: &str,
        tracker: Rc<RefCell<BindResultTracker>>,
    ) {
        let request = BindRequest {
            node_moniker: node.make_component_moniker(),
            node: Rc::downgrade(node),
            driver_url_suffix: driver_url_suffix.to_string(),
            tracker,
            composite_only: false,
        };

        if self.bind_node_set.borrow().is_bind_ongoing() {
            self.pending_bind_requests.borrow_mut().push(request);
            return;
        }

        self.bind_node_set.borrow_mut().remove_orphaned_node(&node.make_component_moniker());
        self.bind_node_set.borrow_mut().start_next_bind_process();

        let weak_self = self.weak_self.clone();
        self.scope.spawn_local(async move {
            if let Some(this) = weak_self.upgrade() {
                this.bind_internal(request).await;
                this.process_pending_bind_requests();
            }
        });
    }

    pub async fn try_bind_all_available(&self) -> Vec<fdd::NodeBindingInfo> {
        if self.bind_node_set.borrow().is_bind_ongoing() {
            let (tx, rx) = oneshot::channel();
            self.pending_orphan_rebind_completers.borrow_mut().push(tx);
            return rx.await.unwrap_or_default();
        }

        if self.bind_node_set.borrow().num_of_available_nodes() == 0 {
            return vec![];
        }

        self.bind_node_set.borrow_mut().start_next_bind_process();

        let (tx, rx) = oneshot::channel();
        let tracker = Rc::new(RefCell::new(BindResultTracker::new(
            self.bind_node_set.borrow().num_of_available_nodes(),
            tx,
        )));

        self.try_bind_all_available_internal(tracker).await;
        let results = rx.await.unwrap_or_default();
        self.process_pending_bind_requests();
        results
    }

    async fn bind_internal(&self, request: BindRequest) {
        assert!(self.bind_node_set.borrow().is_bind_ongoing());
        let node = match request.node.upgrade() {
            Some(node) => node,
            None => {
                warn!("Node was freed before bind request is processed. {}", request.node_moniker);
                request.tracker.borrow_mut().report_no_bind();
                return;
            }
        };

        let mut args =
            fdi::MatchDriverArgs { name: Some(node.name().to_string()), ..Default::default() };

        if *node.node_type() == NodeType::Normal
            && let Some(props) = node.get_node_properties(None)
        {
            args.properties = Some(props);
        }

        if !request.driver_url_suffix.is_empty() {
            args.driver_url_suffix = Some(request.driver_url_suffix.clone());
        }

        let result = self.bridge.request_match_from_driver_index(args).await;

        let node = match request.node.upgrade() {
            Some(node) => node,
            None => {
                warn!("Node was freed before it could be bound");
                request.tracker.borrow_mut().report_no_bind();
                return;
            }
        };

        let bind_result =
            self.bind_node_to_result(&node, request.composite_only, result, true).await;

        let node_moniker = node.make_component_moniker();

        if !bind_result.is_bound()
            && !request.composite_only
            && !self.bind_node_set.borrow().multibind_contains(&node_moniker)
        {
            self.bind_node_set.borrow_mut().add_orphaned_node(&node);
        } else {
            self.bind_node_set.borrow_mut().remove_orphaned_node(&node_moniker);
        }

        if bind_result.is_bound() {
            if let Some(url) = bind_result.driver_url() {
                request.tracker.borrow_mut().report_successful_bind_driver(&node_moniker, url);
            } else if let Some(parents) = bind_result.composite_parents() {
                request
                    .tracker
                    .borrow_mut()
                    .report_successful_bind_composite(&node_moniker, parents);
            } else {
                error!("Unknown bind result type for {}.", node_moniker);
                request.tracker.borrow_mut().report_no_bind();
            }
        } else {
            request.tracker.borrow_mut().report_no_bind();
        }
    }

    async fn bind_node_to_result(
        &self,
        node: &Rc<Node>,
        composite_only: bool,
        result: fidl::Result<fdi::MatchDriverResult>,
        has_tracker: bool,
    ) -> BindResult {
        let matched_driver = match result {
            Ok(res) => res,
            Err(e) => {
                if let fidl::Error::ClientChannelClosed { epitaph, .. } = e {
                    if epitaph != zx::Status::NOT_FOUND || !has_tracker {
                        warn!(
                            "Failed to match Node '{}': {}",
                            node.make_component_moniker(),
                            epitaph
                        );
                    }
                } else {
                    error!("Failed to call match Node '{}': {}", node.name(), e);
                }
                return BindResult::NotBound;
            }
        };

        match matched_driver {
            fdi::MatchDriverResult::Driver(driver_info) => {
                if composite_only
                    || self
                        .bind_node_set
                        .borrow()
                        .multibind_contains(&node.make_component_moniker())
                {
                    return BindResult::NotBound;
                }
                match self.bridge.start_driver(node, driver_info).await {
                    Ok(url) => BindResult::Driver(url),
                    Err(e) => {
                        error!("Failed to start driver '{}': {}", node.name(), e);
                        BindResult::NotBound
                    }
                }
            }
            fdi::MatchDriverResult::CompositeParents(parents) => {
                match self.bind_node_to_spec(node, &parents).await {
                    Ok(bound_parents) => BindResult::Composite(bound_parents),
                    Err(_) => BindResult::NotBound,
                }
            }
            _ => {
                warn!("Unknown MatchDriverResult variant");
                BindResult::NotBound
            }
        }
    }

    async fn bind_node_to_spec(
        &self,
        node: &Rc<Node>,
        parents: &[fdf::CompositeParent],
    ) -> Result<Vec<fdf::CompositeParent>, zx::Status> {
        if node.can_multibind_composites {
            self.bind_node_set.borrow_mut().add_or_move_multibind_node(node);
        }

        let result = self.bridge.bind_to_parent_spec(
            parents,
            Rc::downgrade(node),
            node.can_multibind_composites,
        );
        if let Err(e) = &result {
            if *e != zx::Status::NOT_FOUND {
                error!("Failed to bind node '{}' to any of the matched parent specs.", node.name());
            }
            node.on_match_error(*e);
            return result.map(|_| vec![]);
        }
        let result = result?;

        for composite in result.completed_node_and_drivers {
            let composite_node = composite.node.upgrade().expect("Composite node freed before use");
            if let Err(e) = self
                .bridge
                .start_driver(&composite_node, composite.driver.driver_info.unwrap())
                .await
            {
                error!("Failed to start driver '{}': {}", node.name(), e);
            }
        }

        Ok(result.bound_composite_parents)
    }

    async fn try_bind_all_available_internal(&self, tracker: Rc<RefCell<BindResultTracker>>) {
        assert!(self.bind_node_set.borrow().is_bind_ongoing());
        if self.bind_node_set.borrow().num_of_available_nodes() == 0 {
            return;
        }

        let multibind_nodes: Vec<_> =
            self.bind_node_set.borrow().current_multibind_nodes().into_values().collect();
        for node_weak in multibind_nodes {
            let request = BindRequest {
                node_moniker: node_weak.upgrade().unwrap().make_component_moniker(),
                node: node_weak.clone(),
                driver_url_suffix: "".to_string(),
                tracker: tracker.clone(),
                composite_only: true,
            };
            self.bind_internal(request).await;
        }

        let orphaned_nodes: Vec<_> =
            self.bind_node_set.borrow().current_orphaned_nodes().into_values().collect();
        for node_weak in orphaned_nodes {
            let request = BindRequest {
                node_moniker: node_weak.upgrade().unwrap().make_component_moniker(),
                node: node_weak.clone(),
                driver_url_suffix: "".to_string(),
                tracker: tracker.clone(),
                composite_only: false,
            };
            self.bind_internal(request).await;
        }
    }

    pub fn record_inspect(&self, root: &fuchsia_inspect::Node) {
        root.record_child("orphan_nodes", |orphans| {
            let mut i = 0;
            for (moniker, node_weak) in self.bind_node_set.borrow().current_orphaned_nodes() {
                if node_weak.upgrade().is_some() {
                    orphans.record_child(format!("orphan-{}", i), |orphan| {
                        orphan.record_string("moniker", moniker);
                    });
                    i += 1;
                }
            }

            orphans.record_bool("bind_all_ongoing", self.bind_node_set.borrow().is_bind_ongoing());
            orphans.record_uint(
                "pending_bind_requests",
                self.pending_bind_requests.borrow().len() as u64,
            );
            orphans.record_uint(
                "pending_orphan_rebind_callbacks",
                self.pending_orphan_rebind_completers.borrow().len() as u64,
            );
        });
    }

    pub fn process_pending_bind_requests(&self) {
        assert!(self.bind_node_set.borrow().is_bind_ongoing());
        if self.pending_bind_requests.borrow().is_empty()
            && self.pending_orphan_rebind_completers.borrow().is_empty()
        {
            self.bind_node_set.borrow_mut().end_bind_process();
            return;
        }

        for request in self.pending_bind_requests.borrow().iter() {
            if let Some(node) = request.node.upgrade() {
                self.bind_node_set
                    .borrow_mut()
                    .remove_orphaned_node(&node.make_component_moniker());
            }
        }

        self.bind_node_set.borrow_mut().start_next_bind_process();

        let have_bind_all_orphans_request =
            !self.pending_orphan_rebind_completers.borrow().is_empty();
        let bind_tracker_size = if have_bind_all_orphans_request {
            self.pending_bind_requests.borrow().len()
                + self.bind_node_set.borrow().num_of_available_nodes()
        } else {
            self.pending_bind_requests.borrow().len()
        };

        if have_bind_all_orphans_request && bind_tracker_size == 0 {
            for sender in std::mem::take(&mut *self.pending_orphan_rebind_completers.borrow_mut()) {
                let _ = sender.send(vec![]);
            }
            self.bind_node_set.borrow_mut().end_bind_process();
            return;
        }

        let (tx, rx) = oneshot::channel::<Vec<fdd::NodeBindingInfo>>();
        let weak_self = self.weak_self.clone();
        let completers = std::mem::take(&mut *self.pending_orphan_rebind_completers.borrow_mut());
        self.scope.spawn_local(async move {
            let results = rx.await.unwrap_or_default();
            for completer in completers {
                let _ = completer.send(results.clone());
            }
            if let Some(this) = weak_self.upgrade() {
                this.process_pending_bind_requests();
            }
        });

        let tracker = Rc::new(RefCell::new(BindResultTracker::new(bind_tracker_size, tx)));

        let pending_bind = std::mem::take(&mut *self.pending_bind_requests.borrow_mut());
        let weak_self = self.weak_self.clone();
        self.scope.spawn_local(async move {
            if let Some(this) = weak_self.upgrade() {
                for request in pending_bind {
                    this.bind_internal(request).await;
                }

                if have_bind_all_orphans_request {
                    this.try_bind_all_available_internal(tracker).await;
                }
            }
        });
    }
}

impl BindManagerHandle {
    pub fn new(bridge: Box<dyn BindManagerBridge>) -> Self {
        Self(BindManager::new(bridge))
    }

    pub fn has_ongoing_bind(&self) -> bool {
        self.0.bind_node_set.borrow().is_bind_ongoing()
    }

    pub fn bind(
        &self,
        node: &Rc<Node>,
        driver_url_suffix: &str,
        tracker: Rc<RefCell<BindResultTracker>>,
    ) {
        self.0.bind(node, driver_url_suffix, tracker);
    }

    pub async fn try_bind_all_available(&self) -> Vec<fdd::NodeBindingInfo> {
        self.0.try_bind_all_available().await
    }

    pub fn record_inspect(&self, root: &fuchsia_inspect::Node) {
        self.0.record_inspect(root);
    }
}
