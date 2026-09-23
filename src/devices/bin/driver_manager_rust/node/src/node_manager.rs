// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use async_trait::async_trait;
use driver_manager_driver_host::DriverHost;
use driver_manager_types::BindResultTracker;
use driver_manager_utils::DictionaryUtil;
use fidl_fuchsia_driver_framework as fdf;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

#[async_trait(?Send)]
pub trait NodeManager {
    fn clone_box(&self) -> Box<dyn NodeManager>;
    fn bind(&self, node: &Rc<Node>, tracker: Rc<RefCell<BindResultTracker>>);
    fn bind_to_url(
        &self,
        node: &Rc<Node>,
        driver_url_suffix: &str,
        tracker: Rc<RefCell<BindResultTracker>>,
    );
    fn start_driver(
        &self,
        _node: &Rc<Node>,
        _url: &str,
        _package_type: fdf::DriverPackageType,
    ) -> Result<(), zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    fn get_driver_host(
        &self,
        _driver_host_name_for_colocation: &str,
    ) -> Option<Rc<dyn DriverHost>> {
        None
    }
    async fn create_driver_host(
        &self,
        use_next_vdso: bool,
        driver_host_name_for_colocation: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status>;
    async fn create_driver_host_dynamic_linker(
        &self,
        driver_host_name_for_colocation: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status>;
    async fn destroy_driver_host(
        &self,
        _driver_host_name_for_colocation: String,
    ) -> Result<(), zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
    fn is_test_shutdown_delay_enabled(&self) -> bool;
    fn get_shutdown_test_rng(&self) -> Weak<RefCell<rand::rngs::StdRng>>;
    async fn wait_for_bootup(&self);
    fn get_dictionary_util(&self) -> Result<Rc<DictionaryUtil>, zx::Status>;
    fn memory_attributor(&self) -> Option<Rc<dyn MemoryAttributor>>;
}

pub trait MemoryAttributor {
    fn add_driver(&self, component_token: zx::Event, id: u64, process_koid: zx::Koid);
    fn remove_driver(&self, id: u64);
}
