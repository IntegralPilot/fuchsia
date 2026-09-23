// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

mod binder;
mod fs;
mod objects;
mod process;
mod remote_binder;
mod resource_accessor;
mod shared_memory;
mod tests;
mod thread;
mod trace;
mod user_memory_cursor;

pub use fs::BinderFs;
