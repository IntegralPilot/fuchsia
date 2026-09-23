// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use magma_common_defs;
pub mod magma_driver_base;
pub mod magma_system_buffer;
pub mod magma_system_connection;
pub mod magma_system_context;
pub mod magma_system_device;
pub mod magma_system_semaphore;
pub mod performance_counters_server;
pub mod primary_fidl_server;
pub mod utils;

pub mod traits;

#[cfg(test)]
pub mod mock;
