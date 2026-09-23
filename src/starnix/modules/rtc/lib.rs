// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[allow(clippy::module_inception)]
mod device;
mod file;

/// Initialize an RTC dynamic device.
pub use device::rtc_device_init;
