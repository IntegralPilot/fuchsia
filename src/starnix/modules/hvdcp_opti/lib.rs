// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

mod device;
mod iio_file;
mod qbg_battery_file;
mod qbg_file;
mod utils;

pub use device::hvdcp_opti_init;
