// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl_fuchsia_driver_framework as fdf;

#[async_trait(?Send)]
pub trait CompositeManagerBridge {
    fn box_clone(&self) -> Box<dyn CompositeManagerBridge>;

    async fn bind_nodes_for_composite_node_spec(&self);

    async fn add_spec_to_driver_index(
        &self,
        spec: fdf::CompositeNodeSpec,
    ) -> Result<(), zx::Status>;

    async fn request_rebind_from_driver_index(
        &self,
        spec: String,
        driver_url_suffix: Option<String>,
    ) -> Result<(), zx::Status>;
}
