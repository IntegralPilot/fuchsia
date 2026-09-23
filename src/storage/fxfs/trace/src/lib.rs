// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use fxfs_trace_macros::*;
pub use storage_trace::{self, Id, Scope, TraceFutureExt};

#[macro_export]
macro_rules! duration {
    ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::duration!("fxfs", $name $(,$key => $val)*)
    };
}

#[macro_export]
macro_rules! instant {
    ($name:expr, $scope:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::instant!("fxfs", $name, $scope $(,$key => $val)*)
    };
}

#[macro_export]
macro_rules! flow_begin {
    ($name:expr, $flow_id:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::flow_begin!("fxfs", $name, $flow_id $(,$key => $val)*)
    };
}

#[macro_export]
macro_rules! flow_step {
    ($name:expr, $flow_id:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::flow_step!("fxfs", $name, $flow_id $(,$key => $val)*)
    };
}

#[macro_export]
macro_rules! flow_end {
    ($name:expr, $flow_id:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::flow_end!("fxfs", $name, $flow_id $(,$key => $val)*)
    };
}

#[macro_export]
macro_rules! counter {
    ($name:expr, $counter_id:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::counter!("fxfs", $name, $counter_id $(,$key => $val)*)
    };
}

#[macro_export]
macro_rules! trace_future_args {
    ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::storage_trace::trace_future_args!("fxfs", $name $(,$key => $val)*)
    };
}
