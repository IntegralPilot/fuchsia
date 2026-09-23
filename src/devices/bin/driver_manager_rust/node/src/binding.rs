// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use crate::types::NodeState;
use driver_manager_shutdown::{RemovalSet, ShutdownIntent};
use driver_manager_types::{BindResultTracker, ShutdownState};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use futures::channel::oneshot;
use log::{error, warn};
use std::cell::RefCell;
use std::rc::Rc;

impl Node {
    pub async fn complete_bind(self: &Rc<Self>, result: Result<(), zx::Status>) {
        if result.is_err() {
            warn!("Bind failed for node '{}'", self.make_component_moniker());
            if self.shutdown_state() == ShutdownState::Running && !self.is_unbound() {
                warn!("Quarantining node '{}'", self.make_component_moniker());
                self.quarantine_node().await;
            } else {
                self.set_state(NodeState::Unbound);
            }
        }

        if self.is_running() {
            self.on_bind();
        }

        let completer = self.take_pending_bind_completer();
        if let Some(completer) = completer {
            let _ = completer.send(result);
        }

        if let Err(status) = &result {
            self.on_start_error(*status);
        } else {
            let completer = self.take_wait_for_driver_completer();
            if let Some(completer) = completer {
                if let Some(token) = self.token_handle() {
                    let _ = completer.send(Ok(fdf::DriverResult::DriverStartedNodeToken(token)));
                } else {
                    let _ = completer.send(Err(zx::Status::INTERNAL));
                }
            }
        }

        self.node_shutdown_coordinator.borrow_mut().check_node_state();
    }

    pub(crate) fn wait_for_driver(
        self: &Rc<Self>,
        completer: oneshot::Sender<Result<fdf::DriverResult, zx::Status>>,
    ) {
        if let Some(token) = self.token_handle() {
            let _ = completer.send(Ok(fdf::DriverResult::DriverStartedNodeToken(token)));
            return;
        }

        if self.has_wait_for_driver_completer() {
            let _ = completer.send(Err(zx::Status::ALREADY_EXISTS));
            return;
        }

        self.set_wait_for_driver_completer(completer);

        let node_clone = self.clone();
        self.scope.spawn_local(async move {
            node_clone.node_manager.wait_for_bootup().await;
            let completer = node_clone.take_wait_for_driver_completer();
            if let Some(completer) = completer {
                if let Some(result) = node_clone.bind_error() {
                    let response = match result {
                        fdf::DriverResult::MatchError(s) => Ok(fdf::DriverResult::MatchError(s)),
                        fdf::DriverResult::StartError(s) => Ok(fdf::DriverResult::StartError(s)),
                        _ => Err(zx::Status::INTERNAL),
                    };
                    let _ = completer.send(response);
                    return;
                }

                // Re-check running state
                if let Some(token) = node_clone.token_handle() {
                    let _ = completer.send(Ok(fdf::DriverResult::DriverStartedNodeToken(token)));
                    return;
                }

                let _ = completer.send(Err(zx::Status::INTERNAL));
            }
        });
    }

    pub(crate) async fn bind(self: &Rc<Self>, driver: String) -> Result<(), zx::Status> {
        self.bind_helper(false, Some(driver)).await
    }

    pub(crate) async fn bind_helper(
        self: &Rc<Self>,
        force_rebind: bool,
        driver_url_suffix: Option<String>,
    ) -> Result<(), zx::Status> {
        if !force_rebind && self.is_bound() {
            return Err(zx::Status::ALREADY_BOUND);
        }

        if self.has_pending_bind_completer() {
            return Err(zx::Status::ALREADY_EXISTS);
        }

        let (tx, rx) = oneshot::channel();
        if self.is_bound() {
            self.restart_node_with_rematch(driver_url_suffix, tx);
        } else {
            self.set_pending_bind_completer(tx);
            let tracker = self.create_bind_result_tracker(false);
            if let Some(driver_url_suffix) = driver_url_suffix {
                self.node_manager.bind_to_url(self, &driver_url_suffix, tracker);
            } else {
                self.node_manager.bind(self, tracker);
            }
        }
        rx.await.map_err(|_| zx::Status::INTERNAL)?
    }

    pub(crate) fn create_bind_result_tracker(
        self: &Rc<Self>,
        silent: bool,
    ) -> Rc<RefCell<BindResultTracker>> {
        let weak_self = Rc::downgrade(self);
        let (tx, rx) = oneshot::channel::<Vec<fdd::NodeBindingInfo>>();
        self.scope.spawn_local(async move {
            let Ok(info) = rx.await else {
                return;
            };
            let Some(self_ptr) = weak_self.upgrade() else {
                return;
            };
            if info.is_empty() {
                self_ptr.set_state(NodeState::Unbound);
                self_ptr.on_match_error(zx::Status::NOT_FOUND);
                if !silent {
                    self_ptr.complete_bind(Err(zx::Status::NOT_FOUND)).await;
                }
            } else if info.len() > 1 {
                error!("Unexpectedly bound multiple drivers to a single node");
                self_ptr.on_match_error(zx::Status::BAD_STATE);
                if !silent {
                    self_ptr.complete_bind(Err(zx::Status::BAD_STATE)).await;
                }
            }
        });

        Rc::new(RefCell::new(BindResultTracker::new(1, tx)))
    }

    pub(crate) async fn rebind(self: &Rc<Self>, driver: Option<String>) -> Result<(), zx::Status> {
        let (tx, rx) = oneshot::channel();
        self.restart_node_with_rematch(driver, tx);
        rx.await.map_err(|_| zx::Status::INTERNAL)?
    }

    pub(crate) async fn unbind_children(self: &Rc<Self>) -> Result<(), zx::Status> {
        let children = self.children();
        if children.is_empty() {
            return Ok(());
        }

        let rx = {
            let (tx, rx) = oneshot::channel();
            self.push_unbinding_children_completer(tx);
            if self.unbinding_children_completers_len() == 1 {
                for child in children {
                    child.remove(RemovalSet::All, None);
                }
            }
            rx
        };
        rx.await.map_err(|_| zx::Status::INTERNAL)?
    }

    pub(crate) fn schedule_unbind(self: &Rc<Self>) {
        self.remove(RemovalSet::All, None);
    }

    pub fn restart_node_with_rematch(
        self: &Rc<Self>,
        restart_driver_url_suffix: Option<String>,
        completer: oneshot::Sender<Result<(), zx::Status>>,
    ) {
        if self.has_pending_bind_completer() {
            let _ = completer.send(Err(zx::Status::ALREADY_EXISTS));
            return;
        }

        self.set_pending_bind_completer(completer);
        if let Some(suffix) = restart_driver_url_suffix {
            self.set_restart_driver_url_suffix(suffix);
        }
        self.restart_node();
    }

    pub fn restart_node(self: &Rc<Self>) {
        self.node_shutdown_coordinator.borrow_mut().set_shutdown_intent(ShutdownIntent::Restart);
        self.remove(RemovalSet::All, None);
    }

    pub(crate) async fn quarantine_node(self: &Rc<Self>) {
        if !self.quarantine_start() {
            panic!("QuarantineNode called from unexpected state");
        }

        self.node_shutdown_coordinator.borrow_mut().set_shutdown_intent(ShutdownIntent::Quarantine);
        self.remove(RemovalSet::All, None);
    }

    fn on_bind(&self) {
        if let Some(node_token) = self.token_handle() {
            if let Some(controller_ref) = self.node_controller_ref() {
                let event = fdf::NodeControllerOnBindRequest {
                    node_token: Some(node_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                    ..Default::default()
                };
                if let Err(e) = controller_ref.send_on_bind(event) {
                    error!("Failed to send OnBind event: {}", e);
                }
            }

            let koid = self.token_koid().unwrap().raw_koid();
            if let Some(driver_host) = self.driver_host() {
                let node_manager = self.node_manager.clone_box();
                self.scope.spawn_local(async move {
                    if let Ok(process_koid) = driver_host.get_process_koid().await
                        && let Some(attributor) = node_manager.memory_attributor()
                    {
                        attributor.add_driver(node_token, koid, process_koid);
                    }
                });
            }
        }
    }
}
