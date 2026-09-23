// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! General utilities for converting types.

/// Provides functions for converting infallibly between types.
///
/// This trait can be implemented on types that allow converting between two
/// related types. It has two blanket implementations: `()` for identity
/// conversions, i.e. `Input=Output`, and [`UninstantiableConverter`] as an
/// uninstantiable type that implements the trait for any input and output.
pub trait BidirectionalConverter<Input, Output> {
    /// Converts an instance of `Input` into an instance of `Output`.
    fn convert(&self, a: Input) -> Output;

    /// Converts an instance of `Output` into an instance of `Input`.
    fn convert_back(&self, b: Output) -> Input;
}

impl<I> BidirectionalConverter<I, I> for () {
    fn convert_back(&self, value: I) -> I {
        value
    }
    fn convert(&self, value: I) -> I {
        value
    }
}

/// A marker trait for [`BidirectionalConverter`] of owned or reference types.
pub trait OwnedOrRefsBidirectionalConverter<Input, Output>:
    BidirectionalConverter<Input, Output>
    + for<'a> BidirectionalConverter<&'a Input, &'a Output>
    + for<'a> BidirectionalConverter<&'a mut Input, &'a mut Output>
{
}

impl<I, O, B> OwnedOrRefsBidirectionalConverter<I, O> for B where
    B: BidirectionalConverter<I, O>
        + for<'a> BidirectionalConverter<&'a I, &'a O>
        + for<'a> BidirectionalConverter<&'a mut I, &'a mut O>
{
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn bidirectional_converter_identity() {
        let a = ();
        assert_eq!(a.convert(123), 123);
        assert_eq!(a.convert_back(123), 123);
    }
}
