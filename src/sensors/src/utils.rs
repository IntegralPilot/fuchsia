// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_sensors_types::SensorInfo;

pub fn is_sensor_valid(sensor: &SensorInfo) -> bool {
    return sensor.sensor_id.is_some()
        && sensor.sensor_type.is_some()
        && sensor.wake_up.is_some()
        && sensor.reporting_mode.is_some();
}
