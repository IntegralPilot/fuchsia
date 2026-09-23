// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This belongs in the //src/lib/fidl/rust/fidl_message crate, but has to go
//! here to work around the "upstream crates may add a new impl of trait" error
//! https://doc.rust-lang.org/error_codes/E0119.html.

use crate::encoding::{
    Decode, EmptyPayload, EmptyStruct, Encode, NoHandleResourceDialect, ValueTypeMarker,
};
use crate::persistence::Persistable;

/// Implementation of fidl_message::Body.
pub trait Body:
    Decode<Self::MarkerAtTopLevel, NoHandleResourceDialect>
    + Decode<Self::MarkerInResultUnion, NoHandleResourceDialect>
{
    /// The marker type to use when the body is at the top-level.
    type MarkerAtTopLevel: ValueTypeMarker<Owned = Self>
        + ValueTypeMarker<Owned: Decode<Self::MarkerAtTopLevel, NoHandleResourceDialect>>
        + for<'a> ValueTypeMarker<
            Borrowed<'a>: Encode<Self::MarkerAtTopLevel, NoHandleResourceDialect>,
        >;

    /// The marker type to use when the body is nested in a result union.
    type MarkerInResultUnion: ValueTypeMarker<Owned = Self>
        + ValueTypeMarker<Owned: Decode<Self::MarkerInResultUnion, NoHandleResourceDialect>>
        + for<'a> ValueTypeMarker<
            Borrowed<'a>: Encode<Self::MarkerInResultUnion, NoHandleResourceDialect>,
        >;
}

/// Implementation of fidl_message::ErrorType.
pub trait ErrorType: Decode<Self::Marker, NoHandleResourceDialect> {
    /// The marker type.
    type Marker: ValueTypeMarker<Owned = Self>;
}

impl<T: Persistable> Body for T {
    type MarkerAtTopLevel = Self;
    type MarkerInResultUnion = Self;
}

impl Body for () {
    type MarkerAtTopLevel = EmptyPayload;
    type MarkerInResultUnion = EmptyStruct;
}

impl<E: ValueTypeMarker<Owned = E> + Decode<E, NoHandleResourceDialect>> ErrorType for E {
    type Marker = Self;
}
