// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod execution;
pub mod signal_handling;
pub mod syscalls;
pub mod task;
pub mod vdso;

pub const ARCH_NAME: &'static [u8] = b"x86_64";
pub const ARCH_NAME32: &'static [u8] = b"i686";
