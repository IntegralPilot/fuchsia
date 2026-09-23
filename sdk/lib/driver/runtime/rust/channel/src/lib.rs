// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Bindings for channel functionality in the fuchsia driver framework C API
#![deny(unsafe_op_in_unsafe_fn, missing_docs)]

pub mod arena;
pub mod channel;
pub mod futures;
pub mod message;
pub mod test_utils;
