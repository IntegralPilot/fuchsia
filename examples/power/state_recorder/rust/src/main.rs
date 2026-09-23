// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component::inspector;
use futures::StreamExt;
use state_recorder::{EnumStateRecorder, NumericStateRecorder, RecorderOptions, units};
use strum_macros::{Display, EnumIter, EnumString, FromRepr};

#[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, Eq, PartialEq, Hash, FromRepr)]
#[repr(u8)]
enum ChargingState {
    Discharging = 0,
    Charging = 1,
    FullyCharged = 2,
}

impl From<ChargingState> for u64 {
    fn from(value: ChargingState) -> Self {
        value as Self
    }
}

#[fuchsia::main(logging_tags = ["power_observability", "example"])]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    log::info!("Starting example");

    // Set up tracing
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    // Wait a few seconds to give the user a chance to start collecting a trace.
    fasync::Timer::new(std::time::Duration::from_secs(2)).await;

    let _inspect_server_task =
        inspect_runtime::publish(inspector(), inspect_runtime::PublishOptions::default());

    let mut charging_state_recorder = EnumStateRecorder::new(
        "charging_state".into(),
        c"power_example",
        RecorderOptions { capacity: 10, lazy_record: true, manager: None, persistence: None },
    )
    .expect("DiscreteStateRecorder construction failed");
    let mut battery_level_recorder = NumericStateRecorder::new(
        "battery_level".into(),
        c"power_example",
        units!(Percent),
        Some((0u8, 100)),
        RecorderOptions { lazy_record: false, capacity: 30, manager: None, ..Default::default() },
    )
    .expect("ContinuousStateRecorder construction failed");

    // Simulate a charging interval, followed by an interval at full charge, followed by an interval
    // discharging.
    fasync::Task::local(async move {
        let mut last_charging_state = None;

        for i in 0..25 {
            let (charging_state, battery_level) = match i {
                0..10 => (ChargingState::Charging, 90 + i),
                10..15 => (ChargingState::FullyCharged, 100),
                15..25 => (ChargingState::Discharging, 100 - i + 15),
                _ => unreachable!(),
            };

            if last_charging_state != Some(charging_state) {
                charging_state_recorder.record(charging_state);
            }
            last_charging_state = Some(charging_state);

            battery_level_recorder.record(battery_level);

            fasync::Timer::new(std::time::Duration::from_millis(500)).await;
        }

        // Wait indefinitely to keep recorders from dropping.
        let () = std::future::pending().await;
    })
    .detach();

    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;

    Ok(())
}
