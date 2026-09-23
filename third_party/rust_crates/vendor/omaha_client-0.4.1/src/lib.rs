// Copyright 2019 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

//! This is for Omaha client binaries written in Rust.

#![recursion_limit = "256"]
#![allow(clippy::let_unit_value)]

pub mod app_set;
pub mod async_generator;
pub mod clock;
pub mod common;
pub mod configuration;
pub mod cup_ecdsa;
pub mod http_request;
pub mod http_uri_ext;
pub mod installer;
pub mod metrics;
pub mod policy;
pub mod protocol;
pub mod request_builder;
pub mod state_machine;
pub mod storage;
pub mod time;
pub mod unless;
pub mod version;
