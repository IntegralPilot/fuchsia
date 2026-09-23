// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CompositeManagerBridge, CompositeNodeSpec};
use driver_manager_bind::{BindSpecResult, CompositeNodeAndDriver};
use driver_manager_node::Node;
use futures::channel::oneshot;
use log::{error, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Weak;
use {
    fidl_fuchsia_driver_development as fdd, fidl_fuchsia_driver_framework as fdf,
    fuchsia_async as fasync,
};

pub struct CompositeNodeSpecManager {
    bridge: Box<dyn CompositeManagerBridge>,
    specs: RefCell<HashMap<String, CompositeNodeSpec>>,
}

impl CompositeNodeSpecManager {
    pub fn new(bridge: Box<dyn CompositeManagerBridge>) -> Self {
        Self { bridge, specs: RefCell::new(HashMap::new()) }
    }

    pub async fn add_spec(
        &self,
        fidl_spec: fdf::CompositeNodeSpec,
        spec: CompositeNodeSpec,
    ) -> Result<(), fdf::CompositeNodeSpecError> {
        let name = fidl_spec.name.as_ref().unwrap().clone();
        if self.specs.borrow().contains_key(&name) {
            error!("Duplicate composite node spec {}", name);
            return Err(fdf::CompositeNodeSpecError::AlreadyExists);
        }

        if let Err(e) = self.bridge.add_spec_to_driver_index(fidl_spec).await {
            error!("Failed to add composite node spec to driver index: {}", e);
            return Err(fdf::CompositeNodeSpecError::DriverIndexFailure);
        }

        self.specs.borrow_mut().insert(name, spec);
        let bridge = self.bridge.box_clone();
        fasync::Task::local(async move {
            bridge.bind_nodes_for_composite_node_spec().await;
        })
        .detach();
        Ok(())
    }

    pub fn bind_parent_spec(
        &self,
        composite_parents: &[fdf::CompositeParent],
        node_ptr: Weak<Node>,
        enable_multibind: bool,
    ) -> Result<BindSpecResult, zx::Status> {
        if composite_parents.is_empty() {
            error!("composite_parents needs to contain at least one composite parent.");
            return Err(zx::Status::INVALID_ARGS);
        }

        let mut bound_composite_parents = Vec::new();
        let mut node_and_drivers = Vec::new();

        for composite_parent in composite_parents {
            let composite = match &composite_parent.composite {
                Some(c) => c,
                None => {
                    warn!("CompositeParent missing composite.");
                    continue;
                }
            };

            let index = match composite_parent.index {
                Some(i) => i,
                None => {
                    warn!("CompositeParent missing index.");
                    continue;
                }
            };

            let matched_driver = match &composite.matched_driver {
                Some(md) => md,
                None => continue,
            };

            if matched_driver.composite_driver.is_none() || matched_driver.parent_names.is_none() {
                warn!("CompositeDriverMatch does not have all needed fields.");
                continue;
            }

            let spec_info = match &composite.spec {
                Some(s) => s,
                None => {
                    warn!("CompositeInfo missing spec.");
                    continue;
                }
            };

            let (name, parents) = match (&spec_info.name, &spec_info.parents, &spec_info.parents2) {
                (Some(name), Some(parents), None) => (name, parents.len()),
                (Some(name), None, Some(parents)) => (name, parents.len()),
                _ => {
                    warn!("CompositeNodeSpec missing name or parents.");
                    continue;
                }
            };

            if index as usize >= parents
                || matched_driver.parent_names.as_ref().unwrap().len() != parents
            {
                warn!(
                    "Parent names count does not match the spec parent count or index is out of bounds."
                );
                continue;
            }

            let mut specs = self.specs.borrow_mut();
            let spec = match specs.get_mut(name) {
                Some(s) => s,
                None => {
                    error!("Missing composite node spec {}", name);
                    continue;
                }
            };

            match spec.bind_parent(composite_parent.clone(), node_ptr.clone()) {
                Ok(Some(composite_node)) => {
                    bound_composite_parents.push(composite_parent.clone());
                    if let Some(driver) = &matched_driver.composite_driver {
                        node_and_drivers.push(CompositeNodeAndDriver {
                            driver: driver.clone(),
                            node: composite_node,
                        });
                    }
                }
                Ok(None) => {
                    bound_composite_parents.push(composite_parent.clone());
                }
                Err(zx::Status::ALREADY_BOUND) => {
                    continue;
                }
                Err(e) => {
                    error!("Failed to bind node: {}", e);
                    continue;
                }
            }

            if !enable_multibind {
                break;
            }
        }

        if !bound_composite_parents.is_empty() {
            Ok(BindSpecResult {
                bound_composite_parents,
                completed_node_and_drivers: node_and_drivers,
            })
        } else {
            Err(zx::Status::NOT_FOUND)
        }
    }

    pub async fn rebind(
        &self,
        spec_name: String,
        restart_driver_url_suffix: Option<String>,
    ) -> Result<(), zx::Status> {
        if !self.specs.borrow().contains_key(&spec_name) {
            warn!("Spec {} is not available for rebind", spec_name);
            return Err(zx::Status::NOT_FOUND);
        }

        self.bridge
            .request_rebind_from_driver_index(spec_name.clone(), restart_driver_url_suffix)
            .await?;

        self.on_request_rebind_complete(spec_name).await
    }

    async fn on_request_rebind_complete(&self, spec_name: String) -> Result<(), zx::Status> {
        let rx = {
            let mut specs = self.specs.borrow_mut();
            let spec = specs.get_mut(&spec_name).unwrap();

            let (tx, rx) = oneshot::channel();
            spec.remove(tx);
            rx
        };
        rx.await.map_err(|_| zx::Status::INTERNAL)??;

        log::debug!("Rebinding composite node spec {}", spec_name);
        self.bridge.bind_nodes_for_composite_node_spec().await;
        Ok(())
    }

    pub fn get_composite_info(&self) -> Vec<fdd::CompositeNodeInfo> {
        self.specs.borrow().values().map(|spec| spec.get_composite_info()).collect()
    }
}
