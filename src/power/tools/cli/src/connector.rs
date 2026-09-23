// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use {
    flex_fuchsia_power as fpower, flex_fuchsia_power_manager_debug as fdebug,
    flex_fuchsia_power_topology_test as fpt,
};

pub trait Connector {
    fn get_system_activity_control(
        &self,
    ) -> impl std::future::Future<Output = Result<fpt::SystemActivityControlProxy>>;
    fn get_debug(&self) -> impl std::future::Future<Output = Result<fdebug::DebugProxy>>;
    fn get_reboot_initiator(
        &self,
    ) -> impl std::future::Future<Output = Result<fpower::CollaborativeRebootInitiatorProxy>>;
}
