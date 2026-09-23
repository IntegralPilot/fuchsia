// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Support for running asynchronous objects (streams, tasks, etc.) until stalled.
//!
//! This module simply publishes implementations in child modules.

mod stream;

pub use stream::{
    until_stalled, StallableRequestStream, StreamAndControlHandle, WeakControlHandle,
};
