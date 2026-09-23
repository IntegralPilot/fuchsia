// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(clippy::all, clippy::pedantic, clippy::unwrap_used)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::semicolon_if_nothing_returned
)]

pub mod cobalt;
pub mod debug;
pub mod power;
pub mod session_manager;
pub mod startup;
