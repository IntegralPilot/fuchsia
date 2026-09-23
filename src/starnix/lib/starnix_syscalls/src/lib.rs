// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod syscall_arg;
mod syscall_result;

pub mod decls;

pub use syscall_arg::*;
pub use syscall_result::*;

// This needs to be available to the macros in this library without clients having to depend on
// paste themselves.
#[doc(hidden)]
pub use paste as __paste;
