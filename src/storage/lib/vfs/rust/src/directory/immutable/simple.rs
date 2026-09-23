// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This is an implementation of an immutable "simple" pseudo directories.  Use [`simple()`] to
//! construct actual instances.  See [`Simple`] for details.

#[cfg(test)]
mod tests;

use std::sync::Arc;

pub use crate::directory::simple::Simple;

/// Creates an immutable empty "simple" directory.  This directory holds a "static" set of entries,
/// allowing the server to add or remove entries via the
/// [`crate::directory::helper::DirectlyMutable::add_entry()`] and
/// [`crate::directory::helper::DirectlyMutable::remove_entry()`] methods.
pub fn simple() -> Arc<Simple> {
    Simple::new()
}
