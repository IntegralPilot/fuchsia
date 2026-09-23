// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx::MonotonicDuration;

/// Defines a generic utility to expect calls to a FIDL message and extract their parameters.
pub mod expect;

/// Mock utilities for fuchsia.bluetooth.sys.
pub mod sys;

/// Mock utilities for fuchsia.bluetooth.gatt.
pub mod gatt;

/// Mock utilities for fuchsia.bluetooth.gatt2.
pub mod gatt2;

/// Mock utilities for fuchsia.hardware.bluetooth.
pub mod hci;

/// Mock utilities for fuchsia.bluetooth.le.
pub mod le;

const TIMEOUT_SECONDS: i64 = 4 * 60;

pub fn timeout_duration() -> MonotonicDuration {
    MonotonicDuration::from_seconds(TIMEOUT_SECONDS)
}
