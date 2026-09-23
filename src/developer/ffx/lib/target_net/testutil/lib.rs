// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test utilities for interacting with [`ffx_target_net`].

mod fake_netstack;
mod hyper_connector;

pub use fake_netstack::FakeNetstack;
pub use hyper_connector::TargetHyperConnector;
