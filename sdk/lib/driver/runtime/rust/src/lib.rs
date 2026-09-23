// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Bindings for the fuchsia driver framework C API
#![deny(unsafe_op_in_unsafe_fn, missing_docs)]

// Note: we explicitly export only the parts of the sub-crates that we expect to be used in user
// code to allow for sharing utility and internal access code between these crates.
pub use fdf_channel::arena::*;
pub use fdf_channel::channel::*;
pub use fdf_channel::message::*;
pub use fdf_core::dispatcher::*;
pub use fdf_core::handle::*;

// re-export useful parts of libasync as well
pub use libasync::{
    AfterDeadline, AsAsyncDispatcherRef, AsyncDispatcher, AsyncDispatcherRef, DispatcherTimerExt,
    GetAsyncDispatcher, JoinHandle, OnDispatcher, Task,
};
