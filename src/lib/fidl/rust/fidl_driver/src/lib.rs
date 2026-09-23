// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A parallel library to the rust fidl bindings that supports driver transport
#![deny(unsafe_op_in_unsafe_fn)]

pub mod encoding;
pub mod endpoints;
