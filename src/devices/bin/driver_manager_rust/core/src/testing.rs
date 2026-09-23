// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use driver_manager_driver_host::{DriverHost, DriverLoadArgs, DriverStartArgs, ProcessInfo};
use driver_manager_node::{MemoryAttributor, Node, NodeManager};
use driver_manager_types::BindResultTracker;
use driver_manager_utils::DictionaryUtil;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use {
    fidl_fuchsia_driver_framework as fdf, fidl_fuchsia_driver_host as fdh,
    fidl_fuchsia_ldsvc as fldsvc,
};

pub struct MockDriverHost {
    pub stack_trace_count: AtomicUsize,
}

impl MockDriverHost {
    pub fn new() -> Self {
        Self { stack_trace_count: AtomicUsize::new(0) }
    }
}

impl Default for MockDriverHost {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DriverHost for MockDriverHost {
    async fn start(
        &self,
        _args: DriverStartArgs,
        _driver: fidl::endpoints::ServerEnd<fdh::DriverMarker>,
    ) -> Result<(), zx::Status> {
        Ok(())
    }
    async fn start_with_dynamic_linker(
        &self,
        _load_args: DriverLoadArgs,
        _start_args: DriverStartArgs,
        _driver: fidl::endpoints::ServerEnd<fdh::DriverMarker>,
    ) -> Result<(), zx::Status> {
        Ok(())
    }
    fn install_loader(
        &self,
        _loader: fidl::endpoints::ClientEnd<fldsvc::LoaderMarker>,
    ) -> Result<(), zx::Status> {
        Ok(())
    }
    fn is_dynamic_linking_enabled(&self) -> bool {
        false
    }
    async fn get_process_koid(&self) -> Result<zx::Koid, zx::Status> {
        Ok(zx::Koid::from_raw(0))
    }
    async fn get_process_info_internal(&self) -> Result<ProcessInfo, zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    async fn get_crash_info(
        &self,
        _thread_koid: zx::Koid,
    ) -> Result<fdh::DriverCrashInfo, zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    fn trigger_stack_trace(&self) {
        self.stack_trace_count.fetch_add(1, Ordering::SeqCst);
    }
    fn name_for_colocation(&self) -> &str {
        ""
    }
}

pub struct MockNodeManager;

#[async_trait(?Send)]
impl NodeManager for MockNodeManager {
    fn clone_box(&self) -> Box<dyn NodeManager> {
        Box::new(MockNodeManager)
    }
    fn bind(&self, _node: &Rc<Node>, _tracker: Rc<RefCell<BindResultTracker>>) {}
    fn bind_to_url(&self, _node: &Rc<Node>, _url: &str, _tracker: Rc<RefCell<BindResultTracker>>) {}
    fn start_driver(
        &self,
        _node: &Rc<Node>,
        _url: &str,
        _package_type: fdf::DriverPackageType,
    ) -> Result<(), zx::Status> {
        Ok(())
    }
    fn get_driver_host(&self, _name: &str) -> Option<Rc<dyn DriverHost>> {
        None
    }
    async fn create_driver_host(
        &self,
        _use_next_vdso: bool,
        _name: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    async fn create_driver_host_dynamic_linker(
        &self,
        _name: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    fn is_test_shutdown_delay_enabled(&self) -> bool {
        false
    }
    fn get_shutdown_test_rng(&self) -> Weak<RefCell<rand::rngs::StdRng>> {
        Weak::new()
    }
    async fn wait_for_bootup(&self) {}
    fn get_dictionary_util(&self) -> Result<Rc<DictionaryUtil>, zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    fn memory_attributor(&self) -> Option<Rc<dyn MemoryAttributor>> {
        None
    }
}
