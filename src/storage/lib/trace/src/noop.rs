// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Noop implementations of the functions in `fuchsia_trace` that are called from the tracing
//! macros. These functions are used instead of omitting the calls within the macro to ensure that
//! the macro invocations still compile even when tracing is disabled. The tests in this library are
//! built with tracing enabled and with tracing disabled which will ensure that these functions
//! can't diverge from the real ones.

use std::future::Future;
use std::marker::PhantomData;

#[derive(Copy, Clone)]
pub enum Scope {
    Thread,
    Process,
    Global,
}

#[derive(Clone, Copy)]
pub struct Id(());

impl Id {
    pub fn new() -> Self {
        Self(())
    }
    pub fn random() -> Self {
        Self(())
    }
}

impl From<u64> for Id {
    #[inline]
    fn from(_id: u64) -> Self {
        Self(())
    }
}

#[inline]
pub const fn use_args<'a>(_args: &'a [Arg<'_>]) {}

/// Convenience macro for creating a trace duration event from this macro invocation to the end of
/// the current scope.
///
/// See `fuchsia_trace::duration!` for more details.
#[macro_export]
macro_rules! duration {
    ($category:expr, $name:expr $(, $key:expr => $val:expr)*) => {
        if false {
            $crate::__backend::use_duration_args($category, $name);
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        }
    }
}

#[inline]
pub const fn use_duration_args<'a>(_category: &'static str, _name: &'static str) {}

/// Convenience macro for creating an instant event.
///
/// See `fuchsia_trace::instant!` for more details.
#[macro_export]
macro_rules! instant {
    ($category:expr, $name:expr, $scope:expr $(, $key:expr => $val:expr)*) => {
        if false {
            $crate::__backend::use_instant_args($category, $name, $scope);
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        }
    }
}

#[inline]
pub const fn use_instant_args<'a>(_category: &'static str, _name: &'static str, _scope: Scope) {}

/// Writes a flow begin event with the specified id.
///
/// See `fuchsia_trace::flow_begin!` for more details.
#[macro_export]
macro_rules! flow_begin {
    ($category:expr, $name:expr, $flow_id:expr $(, $key:expr => $val:expr)*) => {
        if false {
            $crate::__backend::use_flow_args($category, $name, $flow_id);
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        }
    }
}

/// Writes a flow step event with the specified id.
///
/// See `fuchsia_trace::flow_step!` for more details.
#[macro_export]
macro_rules! flow_step {
    ($category:expr, $name:expr, $flow_id:expr $(, $key:expr => $val:expr)*) => {
        if false {
            $crate::__backend::use_flow_args($category, $name, $flow_id);
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        }
    }
}

/// Writes a flow end event with the specified id.
///
/// See `fuchsia_trace::flow_end!` for more details.
#[macro_export]
macro_rules! flow_end {
    ($category:expr, $name:expr, $flow_id:expr $(, $key:expr => $val:expr)*) => {
        if false {
            $crate::__backend::use_flow_args($category, $name, $flow_id);
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        }
    }
}

#[inline]
pub const fn use_flow_args(_category: &'static str, _name: &'static str, _flow_id: Id) {}

/// Writes a counter event with the specified id.
///
/// See `fuchsia_trace::counter!` for more details.
#[macro_export]
macro_rules! counter {
    ($category:expr, $name:expr, $counter_id:expr $(, $key:expr => $val:expr)*) => {
        if false {
            $crate::__backend::use_counter_args($category, $name, $counter_id);
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        }
    };
}

#[inline]
pub const fn use_counter_args(_category: &'static str, _name: &'static str, _counter_id: u64) {}

pub trait TraceFutureExt: Future + Sized {
    #[inline(always)]
    fn trace(self, _args: TraceFutureArgs) -> Self {
        self
    }
}

impl<T: Future + Sized> TraceFutureExt for T {}

/// Constructs a `TraceFutureArgs` object to be passed to `TraceFutureExt::trace`.
///
/// See `fuchsia_trace::trace_future_args!` for more details.
#[macro_export]
macro_rules! trace_future_args {
    ($category:expr, $name:expr $(, $key:expr => $val:expr)*) => {{
        if false {
            $crate::__backend::use_args(&[$($crate::__backend::ArgValue::of($key, $val)),*]);
        };
        $crate::__backend::trace_future_args($category, $name)
    }}
}

pub struct TraceFutureArgs {
    pub _use_trace_future_args: (),
}

#[inline]
pub fn trace_future_args<'a>(_category: &'static str, _name: &'static str) -> TraceFutureArgs {
    TraceFutureArgs { _use_trace_future_args: () }
}

pub struct Arg<'a>(PhantomData<&'a ()>);

pub trait ArgValue {
    fn of<'a>(key: &'a str, value: Self) -> Arg<'a>
    where
        Self: 'a;
}

macro_rules! impl_arg_value {
    ($($type:ty),*) => {
        $(
            impl ArgValue for $type {
                #[inline]
                fn of<'a>(_key: &'a str, _value: Self) -> Arg<'a>
                where
                    Self: 'a,
                {
                    Arg(PhantomData)
                }
            }
        )*
    };
}

impl_arg_value!((), bool, i32, u32, i64, u64, isize, usize, f64);

macro_rules! impl_generic_arg_value {
    ($(($type:ty, $generics:tt)),*) => {
        $(
        impl<$generics> ArgValue for $type {
            #[inline]
            fn of<'a>(_key: &'a str, _value: Self) -> Arg<'a>
            where
                Self: 'a,
            {
                Arg(PhantomData)
            }
        }
    )*
    };
}

impl_generic_arg_value!((*const T, T), (*mut T, T), (&'b str, 'b));
