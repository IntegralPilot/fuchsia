// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Library for assembling fuchsia images.
//!
//! See the documentation under each operation for more information
//! on how to use it.

#![deny(missing_docs)]

mod create_system;
mod product;

pub use create_system::{CreateSystemArgs, CreateSystemOutputs};
pub use product::{
    AssemblyMode, ProductArgs, ProductAssemblyOutputs, ValidationMode, default_mode,
};
