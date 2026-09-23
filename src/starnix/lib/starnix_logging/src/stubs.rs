// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use inspect_stubs::*;

/// Initialize the inspect contrib library to be able to associate stub calls with process names.
pub fn register_stub_context_callback() {
    register_context_name_callback(crate::logging::get_current_leader_command);
}
