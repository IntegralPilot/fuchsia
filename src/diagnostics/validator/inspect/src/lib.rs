// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod data; // Inspect data maintainer/updater/scanner-from-vmo; compare engine
mod metrics; // Evaluates memory performance of Inspect library
mod puppet; // Interface to target Inspect library wrapper programs (puppets)
mod results; // Stores and formats reports-to-user
mod runner; // Coordinates testing operations
mod trials; // Defines the trials to run

#[macro_use]
mod macros; // Macros used for common validation actions

pub use runner::run_all_trials;

/// meta/validator.shard.cml must use this name in `children: name:`.
const PUPPET_MONIKER: &str = "puppet";
