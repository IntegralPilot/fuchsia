// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Tools for managing, and allocating, VMARs intended to be passed to thrd_set_zx_create_handles.

mod thread_create_vmars;

pub use thread_create_vmars::*;
