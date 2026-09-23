// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Building a runtime config for component_manager by compiling together platform configs and a
//! product-provided config.

mod compile;
pub use compile::{Args, compile};
