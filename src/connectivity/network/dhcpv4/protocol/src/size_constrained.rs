// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::size_of_contents::SizeOfContents;
use serde::{Deserialize, Serialize};
use std::ops::Deref;

pub const U8_MAX_AS_USIZE: usize = u8::MAX as usize;

/// AtLeast encodes a lower bound on the inner container's number of elements.
///
/// If `inner` is an `AtMostBytes<Vec<U>>`, it must contain at least
/// `LOWER_BOUND_ON_NUMBER_OF_ELEMENTS` instances of `U`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AtLeast<const LOWER_BOUND_ON_NUMBER_OF_ELEMENTS: usize, T> {
    inner: T,
}

// We'd have liked to make this more general, but we run into
// "unconstrained type parameter" issues.
impl<const LOWER_BOUND_ON_NUMBER_OF_ELEMENTS: usize, const UPPER_BOUND_ON_SIZE_IN_BYTES: usize, T>
    TryFrom<AtMostBytes<UPPER_BOUND_ON_SIZE_IN_BYTES, Vec<T>>>
    for AtLeast<
        LOWER_BOUND_ON_NUMBER_OF_ELEMENTS,
        AtMostBytes<UPPER_BOUND_ON_SIZE_IN_BYTES, Vec<T>>,
    >
{
    type Error = (Error, Vec<T>);

    fn try_from(
        value: AtMostBytes<UPPER_BOUND_ON_SIZE_IN_BYTES, Vec<T>>,
    ) -> Result<Self, Self::Error> {
        if value.len() >= LOWER_BOUND_ON_NUMBER_OF_ELEMENTS {
            Ok(AtLeast { inner: value })
        } else {
            let AtMostBytes { inner } = value;
            Err((Error::SizeConstraintViolated, inner))
        }
    }
}

/// AtMostBytes encodes an upper bound on the inner container's size in bytes.
///
/// If `inner` is a Vec<U>, its size in bytes (as defined by
/// `SizeOfContents::size_of_contents_in_bytes`) must be at most
/// `UPPER_BOUND_ON_SIZE_IN_BYTES`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AtMostBytes<const UPPER_BOUND_ON_SIZE_IN_BYTES: usize, T> {
    inner: T,
}

impl<const UPPER_BOUND_ON_SIZE_IN_BYTES: usize, T> TryFrom<Vec<T>>
    for AtMostBytes<UPPER_BOUND_ON_SIZE_IN_BYTES, Vec<T>>
{
    type Error = (Error, Vec<T>);
    fn try_from(value: Vec<T>) -> Result<Self, Self::Error> {
        if value.size_of_contents_in_bytes() <= UPPER_BOUND_ON_SIZE_IN_BYTES {
            Ok(AtMostBytes { inner: value })
        } else {
            Err((Error::SizeConstraintViolated, value))
        }
    }
}

impl<const LOWER_BOUND_ON_NUMBER_OF_ELEMENTS: usize, const UPPER_BOUND_ON_SIZE_IN_BYTES: usize, T>
    TryFrom<Vec<T>>
    for AtLeast<
        LOWER_BOUND_ON_NUMBER_OF_ELEMENTS,
        AtMostBytes<UPPER_BOUND_ON_SIZE_IN_BYTES, Vec<T>>,
    >
{
    type Error = (Error, Vec<T>);

    fn try_from(value: Vec<T>) -> Result<Self, Self::Error> {
        AtLeast::try_from(AtMostBytes::try_from(value)?)
    }
}

impl<const N: usize, T> From<AtMostBytes<N, Vec<T>>> for Vec<T> {
    fn from(value: AtMostBytes<N, Vec<T>>) -> Self {
        let AtMostBytes { inner } = value;
        inner
    }
}

impl<const M: usize, const N: usize, T> From<AtLeast<M, AtMostBytes<N, Vec<T>>>> for Vec<T> {
    fn from(value: AtLeast<M, AtMostBytes<N, Vec<T>>>) -> Self {
        let AtLeast { inner: AtMostBytes { inner } } = value;
        inner
    }
}

macro_rules! impl_for_both {
    ($trait_name:ty, { $($tail:tt)* }) => {
        impl<const N: usize, T> $trait_name for AtMostBytes<N, Vec<T>> {
            $($tail)*
        }

        impl<const M: usize, const N: usize, T> $trait_name for AtLeast<M, AtMostBytes<N, Vec<T>>> {
            $($tail)*
        }
    }
}

impl_for_both!(Deref, {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        let Self { inner } = self;
        inner
    }
});

impl_for_both!(IntoIterator, {
    type Item = T;
    type IntoIter = <Vec<T> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let vec = Vec::from(self);
        vec.into_iter()
    }
});

impl_for_both!(AsRef<[T]>, {
    fn as_ref(&self) -> &[T] {
        self.deref().as_ref()
    }
});

// We can delete a lot of this manual repetition once generic_const_exprs
// is stabilized. https://github.com/rust-lang/rust/issues/76560

trait IsAtMost4Bytes {}

macro_rules! impl_at_most_4_bytes {
    ($t:ty) => {
        impl IsAtMost4Bytes for $t {}
        static_assertions::const_assert!(
            { std::mem::size_of::<$t>() } <= { std::mem::size_of::<u8>() * 4 }
        );
    };
    ($t:ty, $($tail:ty),*) => {
        impl_at_most_4_bytes!($t);
        impl_at_most_4_bytes!($($tail),*);
    }
}

impl_at_most_4_bytes!(u8, u16, u32, crate::OptionCode, std::net::Ipv4Addr);

const U8_MAX_DIVIDED_BY_4: usize = u8::MAX as usize / 4;

enum Num<const X: usize> {}

trait GreaterThanOrEqualTo<const Y: usize> {}

macro_rules! impl_ge {
    ([ $lhs:literal ] >= $rhs:literal) => {
        impl GreaterThanOrEqualTo<$rhs> for Num<$lhs> {}
        ::static_assertions::const_assert!($lhs >= $rhs);
    };
    ([ $lhs:literal, $($ltail:literal),* ] >= $rhs:literal) => {
        impl_ge!([ $lhs ] >= $rhs);

        impl_ge!([ $($ltail),* ] >= $rhs);
    }
}

impl_ge!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10] >= 1);
impl_ge!([2, 3, 4, 5, 6, 7, 8, 9, 10] >= 2);

trait LessThanOrEqualTo<const Y: usize> {}

macro_rules! impl_le {
    ([ $lhs:literal ] <= $rhs:literal) => {
        impl LessThanOrEqualTo<$rhs> for Num<$lhs> {}
        ::static_assertions::const_assert!($lhs <= $rhs);
    };
    ([ $lhs:literal, $($ltail:literal),* ] <= $rhs:literal) => {
        impl_le!([ $lhs ] <= $rhs);

        impl_le!([ $($ltail),* ] <= $rhs);
    }
}

impl_le!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10] <= 63);

impl<T, const LOWER_BOUND_ON_NUMBER_OF_ELEMENTS: usize, const N: usize> From<[T; N]>
    for AtLeast<LOWER_BOUND_ON_NUMBER_OF_ELEMENTS, AtMostBytes<U8_MAX_AS_USIZE, Vec<T>>>
where
    T: IsAtMost4Bytes,
    Num<N>: GreaterThanOrEqualTo<LOWER_BOUND_ON_NUMBER_OF_ELEMENTS>,
    Num<N>: LessThanOrEqualTo<{ U8_MAX_DIVIDED_BY_4 }>,
{
    fn from(value: [T; N]) -> Self {
        value.into_iter().collect::<Vec<_>>().try_into().unwrap_or_else(
            |(Error::SizeConstraintViolated, _)| {
                panic!(
                    "should be statically known that \
                 {N} >= {LOWER_BOUND_ON_NUMBER_OF_ELEMENTS} and \
                 [{type_name}; {N}] fits within {U8_MAX_AS_USIZE} bytes",
                    type_name = std::any::type_name::<T>()
                )
            },
        )
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("size constraint violated")]
    SizeConstraintViolated,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Debug;

    fn run_edge_case<T: Copy + Debug + PartialEq>(item: T, max_number_allowed: impl Into<usize>) {
        let max_number_allowed = max_number_allowed.into();
        let v = std::iter::repeat(item).take(max_number_allowed).collect::<Vec<_>>();
        assert_eq!(
            AtLeast::<1, AtMostBytes<{ U8_MAX_AS_USIZE }, _>>::try_from(v.clone()),
            Ok(AtLeast { inner: AtMostBytes { inner: v } }),
            "{max_number_allowed} instances of {} should fit in 255 bytes",
            std::any::type_name::<T>(),
        );

        let v = std::iter::repeat(item).take(max_number_allowed + 1).collect::<Vec<_>>();
        assert_eq!(
            AtLeast::<1, AtMostBytes<{ U8_MAX_AS_USIZE }, _>>::try_from(v.clone()),
            Err((Error::SizeConstraintViolated, v)),
            "{max_number_allowed} instances of {} should not fit in 255 bytes",
            std::any::type_name::<T>(),
        );
    }

    #[test]
    fn edge_cases() {
        run_edge_case(1u8, u8::MAX);
        run_edge_case(1u32, u8::MAX / 4);
        run_edge_case(1u64, u8::MAX / 8);
    }

    #[test]
    fn disallows_empty() {
        assert_eq!(
            AtLeast::<1, AtMostBytes<{ U8_MAX_AS_USIZE }, _>>::try_from(Vec::<u8>::new()),
            Err((Error::SizeConstraintViolated, Vec::new()))
        )
    }
}
