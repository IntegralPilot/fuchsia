// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use fidl_fuchsia_audio_controller as fac;

pub fn create_reference_clock(clock_type: fac::ClockType) -> Result<Option<zx::Clock>, Error> {
    match clock_type {
        fac::ClockType::Flexible(_) => Ok(None),
        fac::ClockType::SystemMonotonic(_) => {
            let clock = zx::SyntheticClock::create(
                zx::ClockOpts::CONTINUOUS | zx::ClockOpts::AUTO_START,
                None,
            )
            .map_err(|e| anyhow!("Creating reference clock failed: {}", e))?;
            let rights_clock = clock
                .replace_handle(zx::Rights::READ | zx::Rights::DUPLICATE | zx::Rights::TRANSFER)
                .map_err(|e| anyhow!("Replace handle for reference clock failed: {}", e))?;
            Ok(Some(rights_clock))
        }
        fac::ClockType::Custom(info) => {
            let rate = info.rate_adjust;
            let offset = info.offset;
            let now = zx::MonotonicInstant::get();
            let delta_time = now.into_nanos()
                + zx::MonotonicDuration::from_nanos(offset.unwrap_or(0).into()).into_nanos();

            let update_builder = zx::ClockUpdate::builder()
                .rate_adjust(rate.unwrap_or(0))
                .absolute_value(now, zx::SyntheticInstant::from_nanos(delta_time));

            let auto_start =
                if offset.is_some() { zx::ClockOpts::empty() } else { zx::ClockOpts::AUTO_START };

            let clock = zx::SyntheticClock::create(zx::ClockOpts::CONTINUOUS | auto_start, None)
                .map_err(|e| anyhow!("Creating reference clock failed: {}", e))?;

            clock
                .update(update_builder.build())
                .map_err(|e| anyhow!("Updating reference clock failed: {}", e))?;

            Ok(Some(
                clock
                    .replace_handle(zx::Rights::READ | zx::Rights::DUPLICATE | zx::Rights::TRANSFER)
                    .map_err(|e| anyhow!("Replace handle for reference clock failed: {}", e))?,
            ))
        }
        fac::ClockTypeUnknown!() => Ok(None),
    }
}
