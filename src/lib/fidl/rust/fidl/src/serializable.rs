// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Support for `@serializable` types.

/// Trait implemented by `@serializable` types.
pub trait Serializable {
    /// The serialized name.
    const SERIALIZABLE_NAME: &'static str;
}
