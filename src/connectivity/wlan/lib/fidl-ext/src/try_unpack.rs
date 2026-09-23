// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use itertools::Itertools;
use paste::paste;
use std::fmt::Display;

#[derive(Clone, Copy, Debug)]
pub struct NamedField<T, A> {
    pub value: T,
    pub name: A,
}

pub trait WithName<T>: Sized {
    fn with_name<A>(self, name: A) -> NamedField<T, A>;
}

impl<T> WithName<Option<T>> for Option<T> {
    fn with_name<A>(self, name: A) -> NamedField<Option<T>, A> {
        NamedField { value: self, name }
    }
}

/// A trait for attempting to unpack the values of one type into another.
///
/// # Examples
///
/// Basic usage:
///
/// ```
/// impl<T, U> TryUnpack for (Option<T>, Option<U>) {
///     type Unpacked = (T, U);
///     type Error = ();
///
///     fn try_unpack(self) -> Result<Self::Unpacked, Self::Error> {
///         match self {
///             (Some(t), Some(u)) => Ok((t, u)),
///             _ => Err(()),
///         }
///     }
/// }
///
/// assert_eq!(Ok((1i8, 2u8)), (Some(1i8), Some(2u8)).try_unpack());
/// assert_eq!(Err(()), (Some(1i8), None).try_unpack());
/// ```
pub trait TryUnpack {
    type Unpacked;
    type Error;

    /// Tries to unpack value(s) from `self` and returns a `Self::Error` if the operation fails.
    fn try_unpack(self) -> Result<Self::Unpacked, Self::Error>;
}

#[doc(hidden)]
#[macro_export]
macro_rules! one {
    ($_t:tt) => {
        1
    };
}

macro_rules! impl_try_unpack_named_fields {
    (($t:ident, $a:ident)) => {
        impl<$t, $a: Display> TryUnpack
            for NamedField<Option<$t>, $a>
        {
            type Unpacked = $t;
            type Error = anyhow::Error;

            /// Tries to unpack the value a `NamedField<Option<T>, A: Display>`.
            ///
            /// If the `Option<T>` contains a value, then value is unwrapped from the `Option<T>`
            /// and returned, discarding the name.  Otherwise, the returned value is a `Self::Error`
            /// is an array containing the name. An array is returned so that `Self::Error` is an
            /// iterable like in the `TryUnpack` implementation for tuples.
            fn try_unpack(self) -> Result<Self::Unpacked, Self::Error> {
                let NamedField { value, name } = self;
                value.ok_or_else(|| anyhow::format_err!(
                    "Unable to unpack empty field: {}", name
                ))
            }
        }
    };

    ($(($t:ident, $a:ident)),+) => {
        paste!{
            impl<$($t, $a: Display),+> TryUnpack
                for ($(NamedField<Option<$t>, $a>),+)
            {
                type Unpacked = ($($t),+);
                type Error = anyhow::Error;

                /// Tries to unpack values from a tuple of *n* fields each with type
                /// `NamedField<Option<Tᵢ>, Aᵢ: Display>`.
                ///
                /// If every `Option<Tᵢ>` contains a value, then the returned value has the type
                /// `(T₁, T₂, …, Tₙ)`, i.e., every value is unwrapped, and every name is
                /// discarded. Otherwise, the returned value is a `Self::Error` containing name of
                /// each field missing a value.
                fn try_unpack(self) -> Result<Self::Unpacked, Self::Error> {
                    // Fill `names` with the name of each missing field. The variable `arity`
                    // is the arity of the tuple this trait is implemented for.
                    let mut names = None;
                    let names = &mut names;
                    let arity =  0usize $(+ one!($t))+;

                    // Deconstruct the tuple `self` into variables `t1`, `t2`, ...
                    let ($([<$t:lower>]),+) = self;

                    // Rebind each `tn` to an `Option<Tn>` that contains a value if `tn` contains
                    // a success value. If `tn` contains an error value, then append the error
                    // value (name) to `names`.
                    let ($([<$t:lower>]),+) = (
                        $({
                            let NamedField { value, name } = [<$t:lower>];
                            value.or_else(|| {
                                names
                                    .get_or_insert_with(|| Vec::with_capacity(arity))
                                    .push(name.to_string());
                                None
                            })
                        }),+
                    );

                    // If `names` contains a value, then one of the `tn` variables must have
                    // contained an error value. In that case, this function returns the error value
                    // `names`. Otherwise, each `tn` variable is returned without its name.
                    match names.take() {
                        Some(names) => Err(anyhow::format_err!(
                            "Unable to unpack empty field(s): {}",
                            names.into_iter().join(", "),
                        )),
                        _ => Ok(($([<$t:lower>].unwrap()),+)),
                    }
                }
            }
        }
    };
}

impl_try_unpack_named_fields!((T1, A1));
impl_try_unpack_named_fields!((T1, A1), (T2, A2));
impl_try_unpack_named_fields!((T1, A1), (T2, A2), (T3, A3));
impl_try_unpack_named_fields!((T1, A1), (T2, A2), (T3, A3), (T4, A4));
impl_try_unpack_named_fields!((T1, A1), (T2, A2), (T3, A3), (T4, A4), (T5, A5));
