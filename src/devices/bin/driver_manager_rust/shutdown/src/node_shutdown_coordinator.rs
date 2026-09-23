// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node_removal_tracker::{NodeId, NodeInfo, NodeRemovalTracker};
use crate::shutdown_node::ShutdownNode;
use async_trait::async_trait;
use driver_manager_driver_host::DriverHost;
use driver_manager_types::{Collection, ShutdownState};
use fuchsia_async as fasync;
use log::debug;
use rand::RngExt as _;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

// The range of test delay time in milliseconds.
const MIN_TEST_DELAY_MS: u32 = 0;
const MAX_TEST_DELAY_MS: u32 = 5;

#[derive(Clone, Copy)]
pub enum RemovalSet {
    All,
    Package,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum ShutdownIntent {
    Removal,
    Restart,
    RebindComposite,
    Quarantine,
}

#[async_trait(?Send)]
pub trait NodeShutdownBridge {
    fn get_removal_tracker_info(&self, state: ShutdownState) -> NodeInfo;
    fn stop_driver(&self);
    fn stop_driver_component(&self);
    fn is_pending_bind(&self) -> bool;
    fn has_children(&self) -> bool;
    fn has_driver(&self) -> bool;
    fn has_driver_component(&self) -> bool;
    fn collection(&self) -> Collection;
    fn children(&self) -> Vec<Rc<dyn ShutdownNode>>;
    fn get_weak_node(&self) -> Weak<dyn ShutdownNode>;
    fn has_driver_component_controller(&self) -> bool;
    fn maybe_destroy_driver_component(&self, intent: ShutdownIntent) -> bool;
    fn get_driver_host(&self) -> Option<Rc<dyn DriverHost>>;
}

pub struct NodeShutdownCoordinator {
    pub(crate) bridge: Box<dyn NodeShutdownBridge>,
    enable_test_shutdown_delays: bool,
    rng_gen: Weak<RefCell<rand::rngs::StdRng>>,
    removal_id: Option<NodeId>,
    pub(crate) removal_tracker: Option<Weak<RefCell<NodeRemovalTracker>>>,
    pub(crate) node_state: ShutdownState,
    pub shutdown_intent: ShutdownIntent,
    pub(crate) pending_task: Option<fasync::Task<()>>,
}

impl NodeShutdownCoordinator {
    pub fn new(
        bridge: Box<dyn NodeShutdownBridge>,
        enable_test_shutdown_delays: bool,
        rng_gen: Weak<RefCell<rand::rngs::StdRng>>,
    ) -> Self {
        Self {
            bridge,
            enable_test_shutdown_delays,
            rng_gen,
            removal_id: None,
            removal_tracker: None,
            node_state: ShutdownState::Running,
            shutdown_intent: ShutdownIntent::Removal,
            pending_task: None,
        }
    }

    pub fn remove(
        node: Rc<dyn ShutdownNode>,
        mut removal_set: RemovalSet,
        removal_tracker: Option<Weak<RefCell<NodeRemovalTracker>>>,
    ) {
        let mut nodes_to_check: Vec<Rc<dyn ShutdownNode>> = vec![];
        let mut stack = vec![(node, removal_set)];

        while let Some((current_node, current_set)) = stack.pop() {
            let mut coordinator = current_node.get_shutdown_coordinator();
            if let Some(tracker) = &removal_tracker {
                coordinator.set_removal_tracker(tracker.clone());
            }

            debug!(
                "Remove called on Node: {} state {:?}",
                current_node.name(),
                coordinator.node_state
            );

            match (coordinator.node_state, current_set) {
                (ShutdownState::Prestop, RemovalSet::Package) => continue,
                (ShutdownState::Running | ShutdownState::Stopped, RemovalSet::Package)
                    if matches!(
                        coordinator.bridge.collection(),
                        Collection::Boot | Collection::None
                    ) =>
                {
                    coordinator.node_state = ShutdownState::Prestop;
                }
                (ShutdownState::Running | ShutdownState::Prestop | ShutdownState::Stopped, _) => {
                    coordinator.node_state = ShutdownState::WaitingOnDriverBind;
                    removal_set = RemovalSet::All;
                }
                _ => continue,
            }

            coordinator.notify_removal_tracker();

            for child in coordinator.bridge.children() {
                child.set_should_destroy_driver_component(true);
                stack.push((child, removal_set));
            }
            nodes_to_check.push(current_node.clone());
        }

        for node in nodes_to_check {
            node.get_shutdown_coordinator().check_node_state();
        }
    }

    pub fn check_node_state(&mut self) {
        if self.pending_task.is_some() {
            return;
        }
        match self.node_state {
            ShutdownState::Running | ShutdownState::Prestop | ShutdownState::Destroyed => {}
            ShutdownState::Stopped => self.check_stopped(),
            ShutdownState::WaitingOnDestroy => self.check_waiting_on_destroy(),
            ShutdownState::WaitingOnDriverBind => self.check_waiting_on_driver_bind(),
            ShutdownState::WaitingOnChildren => self.check_waiting_on_children(),
            ShutdownState::WaitingOnDriver => self.check_waiting_on_driver(),
            ShutdownState::WaitingOnDriverComponent => self.check_waiting_on_driver_component(),
        }
    }

    fn check_waiting_on_driver_bind(&mut self) {
        if self.bridge.is_pending_bind() {
            return;
        }
        self.perform_transition(ShutdownState::WaitingOnChildren);
    }

    fn check_waiting_on_children(&mut self) {
        if self.bridge.has_children() {
            return;
        }
        self.perform_transition(ShutdownState::WaitingOnDriver);
    }

    fn check_waiting_on_driver(&mut self) {
        if self.bridge.has_driver() {
            return;
        }
        self.perform_transition(ShutdownState::WaitingOnDriverComponent);
    }

    fn check_waiting_on_driver_component(&mut self) {
        if self.bridge.has_driver_component() {
            return;
        }

        self.perform_transition(ShutdownState::Stopped);
    }

    fn check_stopped(&mut self) {
        self.perform_transition(ShutdownState::WaitingOnDestroy);
    }

    fn check_waiting_on_destroy(&mut self) {
        if self.bridge.has_driver_component_controller() {
            return;
        }

        self.perform_transition(ShutdownState::Destroyed);
    }

    fn perform_transition(&mut self, next_state: ShutdownState) {
        if self.pending_task.is_some() {
            return;
        }

        let weak_node = self.bridge.get_weak_node();
        let action = async move {
            if let Some(node) = weak_node.upgrade() {
                let shutdown_intent = {
                    let mut coordinator = node.get_shutdown_coordinator();
                    let shutdown_intent = coordinator.shutdown_intent;
                    match next_state {
                        ShutdownState::WaitingOnChildren => {} // No action before state change
                        ShutdownState::WaitingOnDriver => coordinator.bridge.stop_driver(),
                        ShutdownState::WaitingOnDriverComponent => {
                            coordinator.bridge.stop_driver_component()
                        }
                        ShutdownState::Stopped => {} // No action before state change
                        ShutdownState::WaitingOnDestroy => {
                            if coordinator.bridge.has_driver_component_controller()
                                && !coordinator
                                    .bridge
                                    .maybe_destroy_driver_component(shutdown_intent)
                            {
                                // Not ready to transition to WaitingOnDestroy if we haven't sent
                                // the destroy request yet.
                                drop(coordinator.pending_task.take());
                                return;
                            }
                        }
                        ShutdownState::Destroyed => {} // No action before state change
                        _ => panic!("Invalid state for perform_transition"),
                    }
                    shutdown_intent
                };

                if next_state == ShutdownState::Destroyed {
                    if shutdown_intent != ShutdownIntent::Restart
                        && shutdown_intent != ShutdownIntent::Quarantine
                    {
                        node.finish_shutdown().await;
                    }

                    node.schedule_post_shutdown(shutdown_intent);
                }

                node.get_shutdown_coordinator().update_and_notify_state(next_state);
            }
        };

        if let Some(delay_ms) = self.generate_test_delay_ms() {
            self.pending_task = Some(fasync::Task::local(async move {
                fasync::Timer::new(zx::MonotonicDuration::from_millis(delay_ms as i64)).await;
                action.await;
            }));
        } else {
            self.pending_task = Some(fasync::Task::local(async move {
                action.await;
            }));
        }
    }

    fn generate_test_delay_ms(&mut self) -> Option<u32> {
        if !self.enable_test_shutdown_delays {
            return None;
        }
        let binding = self.rng_gen.upgrade()?;
        let mut rng = binding.borrow_mut();
        if rng.random_range(0..5) == 1 {
            Some(rng.random_range(MIN_TEST_DELAY_MS..=MAX_TEST_DELAY_MS))
        } else {
            None
        }
    }

    pub(crate) fn update_and_notify_state(&mut self, state: ShutdownState) {
        self.node_state = state;
        if let Some(task) = self.pending_task.take() {
            drop(task.abort());
        }

        self.notify_removal_tracker();
        self.check_node_state();
    }

    pub(crate) fn notify_removal_tracker(&self) {
        if let (Some(tracker_weak), Some(id)) = (&self.removal_tracker, self.removal_id)
            && let Some(tracker) = tracker_weak.upgrade()
        {
            tracker.borrow_mut().notify(id, self.node_state, tracker_weak.clone());
        }
    }

    pub fn node_state(&self) -> &ShutdownState {
        &self.node_state
    }

    pub fn set_removal_tracker(&mut self, tracker: Weak<RefCell<NodeRemovalTracker>>) {
        if self.removal_tracker.is_none()
            && let Some(tracker) = tracker.upgrade()
        {
            self.removal_id = Some(
                tracker
                    .borrow_mut()
                    .register_node(self.bridge.get_removal_tracker_info(self.node_state)),
            );

            self.removal_tracker = Some(Rc::downgrade(&tracker));
        }
    }

    pub fn set_shutdown_intent(&mut self, intent: ShutdownIntent) {
        self.shutdown_intent = intent;
    }

    pub fn reset_shutdown(&mut self) {
        self.node_state = ShutdownState::Running;
        self.shutdown_intent = ShutdownIntent::Removal;
    }

    pub fn is_shutting_down(&self) -> bool {
        self.node_state != ShutdownState::Running && self.node_state != ShutdownState::Prestop
    }
}
