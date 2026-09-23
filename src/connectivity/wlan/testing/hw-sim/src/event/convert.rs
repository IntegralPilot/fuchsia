// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fallible conversions.
//!
//! This module abstracts over branching (fallible) types like [`Option`] and [`Result`] and is a
//! stop-gap for traits in the Rust core library that are unstable at time of writing (namely
//! `core::ops::Try`). The [`Try`] trait allows event handlers to compose such types in their
//! outputs. For example, [`Handler::expect`] is available when the output of a handler implements
//! [`Try`].
//!
//! See the documentation for `core::ops::Try` etc. for more about how this mechanism works:
//! https://doc.rust-lang.org/std/ops/trait.Try.html#
//!
//! [`Handler::expect`]: crate::event::handler::expect
//! [`Option`]: core::option::Option
//! [`Result`]: core::result::Result
//! [`Try`]: crate::event::Try

// TODO(https://fxbug.dev/42078844): Implement this in terms of the standard `core::ops::Try` and related
//                         traits when they stabilize. Simplify (and eventually remove) this module
//                         when those APIs can be used instead. Note that `core::ops::Try` is
//                         already be implemented for `Option` and `Result`, so `core::ops::Try`
//                         and `core::ops::FromResidual` must only be implemented for `Handled`.

use std::convert::Infallible;
use std::fmt::Display;
use std::ops::ControlFlow;

// This trait provides methods that (at time of writing) are unstable. They must be invoked using
// fully qualified syntax since they share the same names as their unstable counterparts.
pub trait ControlFlowExt<B, C> {
    // TODO(https://fxbug.dev/335283785): Remove or document unused code
    #[allow(dead_code)]
    fn map_break<T, F>(self, f: F) -> ControlFlow<T, C>
    where
        F: FnOnce(B) -> T;

    // TODO(https://fxbug.dev/335283785): Remove or document unused code
    #[allow(dead_code)]
    fn map_continue<T, F>(self, f: F) -> ControlFlow<B, T>
    where
        F: FnOnce(C) -> T;
}

impl<B, C> ControlFlowExt<B, C> for ControlFlow<B, C> {
    fn map_break<T, F>(self, f: F) -> ControlFlow<T, C>
    where
        F: FnOnce(B) -> T,
    {
        match self {
            ControlFlow::Break(inner) => ControlFlow::Break(f(inner)),
            ControlFlow::Continue(inner) => ControlFlow::Continue(inner),
        }
    }

    fn map_continue<T, F>(self, f: F) -> ControlFlow<B, T>
    where
        F: FnOnce(C) -> T,
    {
        match self {
            ControlFlow::Break(inner) => ControlFlow::Break(inner),
            ControlFlow::Continue(inner) => ControlFlow::Continue(f(inner)),
        }
    }
}

// This trait resembles the (at time of writing) unstable `core::ops::Try` and
// `core::ops::FromResidual` traits. While it does not enable the `?` operator, it provides the
// same abstraction of branching types and is used to enable APIs like `Handler::expect` and
// `Handler::try_and`.
/// Divergent (fallible) types.
///
/// This trait abstracts the branching of types that represent independent outputs and non-outputs
/// (errors). Such types must be constructible from these variants and queried for a corresponding
/// control flow.
pub trait Try {
    type Output;
    // See the documentation for `core::ops::Try::Residual` for more about this associated type:
    // https://doc.rust-lang.org/std/ops/trait.Try.html#associatedtype.Residual
    type Residual;

    fn from_output(output: Self::Output) -> Self;

    fn from_residual(residual: Self::Residual) -> Self;

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output>;

    fn unwrap(self) -> Self::Output
    where
        Self: Sized,
    {
        match self.branch() {
            ControlFlow::Continue(output) => output,
            _ => panic!(),
        }
    }

    fn expect(self, message: impl Display) -> Self::Output
    where
        Self: Sized,
    {
        match self.branch() {
            ControlFlow::Continue(output) => output,
            _ => panic!("{}", message),
        }
    }
}

impl<T> Try for Option<T> {
    type Output = T;
    type Residual = Option<Infallible>;

    fn from_output(output: Self::Output) -> Self {
        Some(output)
    }

    fn from_residual(_: Self::Residual) -> Self {
        None
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            Some(inner) => ControlFlow::Continue(inner),
            _ => ControlFlow::Break(None),
        }
    }
}

impl<T, E> Try for Result<T, E> {
    type Output = T;
    type Residual = Result<Infallible, E>;

    fn from_output(output: Self::Output) -> Self {
        Ok(output)
    }

    fn from_residual(residual: Self::Residual) -> Self {
        match residual {
            Err(error) => Err(error),
            _ => unreachable!(),
        }
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            Ok(inner) => ControlFlow::Continue(inner),
            Err(error) => ControlFlow::Break(Err(error)),
        }
    }
}
