// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node_shutdown_coordinator::{NodeShutdownCoordinator, ShutdownIntent};
use async_trait::async_trait;
use std::cell::RefMut;

#[async_trait(?Send)]
pub trait ShutdownNode {
    fn get_shutdown_coordinator(&self) -> RefMut<'_, NodeShutdownCoordinator>;
    fn name(&self) -> &str;
    async fn finish_shutdown(&self);
    fn schedule_post_shutdown(&self, intent: ShutdownIntent);
    fn set_should_destroy_driver_component(&self, val: bool);
}
