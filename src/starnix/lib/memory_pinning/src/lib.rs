// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Tools for ensuring that memory managed by Starnix stays resident under memory pressure.

mod shadow_process;

pub use shadow_process::*;
