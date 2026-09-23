// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FromWire;

/// Associates a good default type for a wire type to convert into.
pub trait IntoNatural: Sized {
    /// A good default type for this wire type to convert into.
    type Natural: FromWire<Self>;

    /// Converts this type into its natural equivalent.
    fn into_natural(self) -> Self::Natural {
        Self::Natural::from_wire(self)
    }
}

impl<T: IntoNatural, const N: usize> IntoNatural for [T; N] {
    type Natural = [T::Natural; N];
}
