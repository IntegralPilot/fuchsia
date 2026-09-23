// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::result_large_err)]
#![allow(clippy::let_unit_value)]

pub mod body;
pub mod range;
pub mod repo_builder;
pub mod repo_keys;
pub mod repository;
pub mod resource;
pub mod test_utils;
pub mod util;

#[cfg(not(target_os = "fuchsia"))]
pub mod manager;
#[cfg(not(target_os = "fuchsia"))]
pub mod package_manifest_watcher;
#[cfg(not(target_os = "fuchsia"))]
pub mod repo_client;
#[cfg(not(target_os = "fuchsia"))]
pub mod server;
