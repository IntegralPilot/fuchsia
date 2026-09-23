// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::LazyLock;
use std::time::Duration;

pub const CPU_SAMPLE_PERIOD: Duration = Duration::from_secs(60);
pub const COMPONENT_CPU_MAX_SAMPLES: usize = 60;
pub const MAX_DEAD_TASKS: usize = 90;
pub const TIMESTAMPS: &'static str = "timestamps";
pub const CPU_TIMES: &'static str = "cpu_times";
pub const QUEUE_TIMES: &'static str = "queue_times";

pub static MEASUREMENT_EPSILON: LazyLock<Duration> = LazyLock::new(|| CPU_SAMPLE_PERIOD / 6);
