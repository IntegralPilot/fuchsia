// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Keymap configuration store.
//!
//! Used to load and store keymap configurations from configuration files.

#[derive(Debug, Clone)]
pub struct Metadata {
    /// The identifier for this keymap. Should be unique across all accessible
    /// keymaps.  Obviously, we'll need to work on that.
    pub keymap_id: String,
}
