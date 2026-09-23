// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provider for a libasync dispatcher abstraction that uses a fuchsia-async Scope as its underlying
//! engine.
#![deny(unsafe_op_in_unsafe_fn, missing_docs)]

mod ops;
mod scope_dispatcher;

pub use scope_dispatcher::*;
