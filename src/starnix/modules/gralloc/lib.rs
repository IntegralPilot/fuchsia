// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

#[allow(clippy::module_inception)]
mod device;
mod file;

pub use device::gralloc_device_init;
