// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::Cow;

/// The ID of a vertex.
pub trait VertexId {
    /// Fetches the ID of a vertex, which must have a string representation.
    fn get_id(&self) -> Cow<'_, str>;
}

impl<T: std::fmt::Display> VertexId for T {
    fn get_id(&self) -> Cow<'_, str> {
        Cow::Owned(format!("{self}"))
    }
}

impl VertexId for str {
    fn get_id(&self) -> Cow<'_, str> {
        Cow::Borrowed(self)
    }
}
