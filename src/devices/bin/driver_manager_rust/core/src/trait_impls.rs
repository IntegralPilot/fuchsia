// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{DriverRunner, DriverRunnerBridge};
use async_trait::async_trait;
use driver_manager_bind::{BindManagerBridge, BindSpecResult};
use driver_manager_composite::CompositeManagerBridge;
use driver_manager_driver_host::DriverHost;
use driver_manager_node::{Node, NodeManager};
use driver_manager_shutdown::{NodeRemover, RemovalSet};
use driver_manager_types::BindResultTracker;
use driver_manager_utils::DictionaryUtil;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_index as fdi;
use futures::channel::oneshot;
use log::info;
use rand::rngs::StdRng;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

#[async_trait(?Send)]
impl NodeRemover for DriverRunner {
    async fn shutdown_all_drivers(&self) {
        info!("Driver Runner invokes shutdown all drivers");
        let (tx, rx) = oneshot::channel();
        self.removal_tracker.borrow_mut().set_all_callback(tx);
        self.root_node.remove(RemovalSet::All, Some(Rc::downgrade(&self.removal_tracker)));
        self.removal_tracker.borrow_mut().finish_enumeration(Rc::downgrade(&self.removal_tracker));
        let _ = rx.await;
    }

    async fn shutdown_pkg_drivers(&self) {
        let (tx, rx) = oneshot::channel();
        self.removal_tracker.borrow_mut().set_pkg_callback(tx);
        self.root_node.remove(RemovalSet::Package, Some(Rc::downgrade(&self.removal_tracker)));
        self.removal_tracker.borrow_mut().finish_enumeration(Rc::downgrade(&self.removal_tracker));
        let _ = rx.await;
    }

    fn set_on_removal_timeout_callback(&self, callback: Box<dyn Fn()>) {
        self.removal_tracker.borrow_mut().set_on_removal_timeout_callback(callback);
    }
}

#[async_trait(?Send)]
impl NodeManager for DriverRunnerBridge {
    fn clone_box(&self) -> Box<dyn NodeManager> {
        Box::new(DriverRunnerBridge(self.0.clone()))
    }

    fn bind(&self, node: &Rc<Node>, tracker: Rc<RefCell<BindResultTracker>>) {
        if let Some(runner) = self.0.upgrade() {
            runner.bind_manager.bind(node, "", tracker);
        }
    }

    fn bind_to_url(
        &self,
        node: &Rc<Node>,
        driver_url_suffix: &str,
        tracker: Rc<RefCell<BindResultTracker>>,
    ) {
        if let Some(runner) = self.0.upgrade() {
            runner.bind_manager.bind(node, driver_url_suffix, tracker);
        }
    }

    fn start_driver(
        &self,
        node: &Rc<Node>,
        url: &str,
        package_type: fdf::DriverPackageType,
    ) -> Result<(), zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            let node_clone = node.clone();
            let url_clone = url.to_string();
            let runner_clone = runner.clone();

            // Schedule the start logic to run later, which helps keep the scope under driver runner
            // as opposed to the node calling this.
            runner.scope.spawn_local(async move {
                let _ = runner_clone.start_driver(&node_clone, &url_clone, package_type).await;
            });
            Ok(())
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    fn get_driver_host(&self, driver_host_name_for_colocation: &str) -> Option<Rc<dyn DriverHost>> {
        let runner = self.0.upgrade()?;
        runner.get_driver_host(driver_host_name_for_colocation)
    }

    async fn create_driver_host(
        &self,
        use_next_vdso: bool,
        driver_host_name_for_colocation: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            runner.create_driver_host(use_next_vdso, driver_host_name_for_colocation).await
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    async fn create_driver_host_dynamic_linker(
        &self,
        driver_host_name_for_colocation: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            runner.create_driver_host_dynamic_linker(driver_host_name_for_colocation).await
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    async fn destroy_driver_host(
        &self,
        driver_host_name_for_colocation: String,
    ) -> Result<(), zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            runner.destroy_driver_host(driver_host_name_for_colocation).await
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    fn is_test_shutdown_delay_enabled(&self) -> bool {
        if let Some(runner) = self.0.upgrade() { runner.enable_test_shutdown_delays } else { false }
    }

    fn get_shutdown_test_rng(&self) -> Weak<RefCell<StdRng>> {
        if let Some(runner) = self.0.upgrade() {
            Rc::downgrade(&runner.shutdown_test_rng)
        } else {
            Weak::new()
        }
    }

    async fn wait_for_bootup(&self) {
        if let Some(runner) = self.0.upgrade() {
            runner.bootup_tracker.wait_for_bootup().await;
        }
    }

    fn get_dictionary_util(&self) -> Result<Rc<DictionaryUtil>, zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            Ok(runner.dictionary_util.clone())
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    fn memory_attributor(&self) -> Option<Rc<dyn driver_manager_node::MemoryAttributor>> {
        let runner = self.0.upgrade()?;
        Some(runner.memory_attributor.clone())
    }
}

#[async_trait(?Send)]
impl BindManagerBridge for DriverRunnerBridge {
    fn box_clone(&self) -> Box<dyn BindManagerBridge> {
        Box::new(Self(self.0.clone()))
    }

    fn on_binding_state_changed(&self) {
        if let Some(runner) = self.0.upgrade() {
            runner.bootup_tracker.notify_binding_changed();
        }
    }

    async fn request_match_from_driver_index(
        &self,
        args: fidl_fuchsia_driver_index::MatchDriverArgs,
    ) -> fidl::Result<fdi::MatchDriverResult> {
        if let Some(runner) = self.0.upgrade() {
            match runner.driver_index.match_driver(&args).await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(fidl::Error::ClientChannelClosed {
                    epitaph: fidl::Epitaph::Explicit(Err(zx::Status::err_from_raw(e))),
                    protocol_name: fdi::DriverIndexMarker::PROTOCOL_NAME,
                }),
                Err(e) => Err(e),
            }
        } else {
            Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Err(zx::Status::UNAVAILABLE)),
                protocol_name: fdi::DriverIndexMarker::PROTOCOL_NAME,
            })
        }
    }

    async fn start_driver(
        &self,
        node: &Rc<Node>,
        driver_info: fidl_fuchsia_driver_framework::DriverInfo,
    ) -> Result<String, zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            let url = driver_info.url.clone().ok_or(zx::Status::INVALID_ARGS)?;
            let package_type = driver_info.package_type.unwrap_or(fdf::DriverPackageType::Base);

            let node_clone = node.clone();
            let url_clone = url.clone();
            let runner_clone = runner.clone();

            // Schedule the start logic to run later, this is because the bind manager reports
            // matching success/failure after calling this function, so we don't want it to report
            // on actual driver start results, as we report that inside start_driver.
            runner.scope.spawn_local(async move {
                let _ = runner_clone.start_driver(&node_clone, &url_clone, package_type).await;
            });
            Ok(url)
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }

    fn bind_to_parent_spec(
        &self,
        parents: &[fdf::CompositeParent],
        node: Weak<Node>,
        enable_multibind: bool,
    ) -> Result<BindSpecResult, zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            runner.composite_node_spec_manager.bind_parent_spec(parents, node, enable_multibind)
        } else {
            Err(zx::Status::UNAVAILABLE)
        }
    }
}

#[async_trait(?Send)]
impl CompositeManagerBridge for DriverRunnerBridge {
    fn box_clone(&self) -> Box<dyn CompositeManagerBridge> {
        Box::new(Self(self.0.clone()))
    }

    async fn bind_nodes_for_composite_node_spec(&self) {
        if let Some(runner) = self.0.upgrade() {
            let _ = runner.bind_manager.try_bind_all_available().await;
        }
    }

    async fn add_spec_to_driver_index(
        &self,
        spec: fidl_fuchsia_driver_framework::CompositeNodeSpec,
    ) -> Result<(), zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            runner
                .driver_index
                .add_composite_node_spec(&spec)
                .await
                .map_err(|_| zx::Status::INTERNAL)?
                .map_err(zx::Status::err_from_raw)
        } else {
            Err(zx::Status::INTERNAL)
        }
    }

    async fn request_rebind_from_driver_index(
        &self,
        spec: String,
        driver_url_suffix: Option<String>,
    ) -> Result<(), zx::Status> {
        if let Some(runner) = self.0.upgrade() {
            runner
                .driver_index
                .rebind_composite_node_spec(&spec, driver_url_suffix.as_deref())
                .await
                .map_err(|_| zx::Status::INTERNAL)?
                .map_err(zx::Status::err_from_raw)
        } else {
            Err(zx::Status::INTERNAL)
        }
    }
}
