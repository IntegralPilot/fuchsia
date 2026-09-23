// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IP layer integration tests.

#![cfg(test)]
#![no_std]
#![warn(missing_docs, unreachable_patterns, clippy::useless_conversion, clippy::redundant_clone)]
// TODO(https://fxbug.dev/339502691): Return to the default limit once lock
// ordering no longer causes overflows.
#![recursion_limit = "256"]

extern crate alloc;

mod device {
    mod base;
    mod ndp;
    mod nud;
    mod route_discovery;
    mod slaac;
}

mod base;
mod forwarding;
mod gmp;
mod icmp;
mod socket;
