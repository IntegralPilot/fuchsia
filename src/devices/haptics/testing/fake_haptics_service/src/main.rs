// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The fake haptics service's goal is to provide a component that offers a fake
//! fuchsia.hardware.haptics.Service` FIDL service instance.

use std::time::Duration;

use anyhow::Context;
use fidl_fuchsia_hardware_haptics::{
    CompositeEffectWaveform, DeviceRequest, DeviceRequestStream, Effect, ServiceRequest,
    SupportedCompositeEffectWaveform, SupportedEffect,
};
use fuchsia_async::Timer;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryFutureExt, TryStreamExt};
use log::error;

async fn handle_device_requests(mut requests: DeviceRequestStream) -> anyhow::Result<()> {
    // Value must be greater than 0 in order to pass tests.
    const COMPOSITE_EFFECT_DURATION: Duration = Duration::from_millis(1);

    while let Some(request) = requests.try_next().await.context("Failed to get request")? {
        match request {
            DeviceRequest::StartVibration { responder } => {
                responder.send(Ok(())).context("Failed to send response")?;
            }
            DeviceRequest::PlayVibration { duration: _, responder } => {
                responder.send(Ok(())).context("Failed to send response")?;
            }
            DeviceRequest::PlayEffect { effect: _, strength: _, responder } => {
                responder.send(Ok(())).context("Failed to send response")?;
            }
            DeviceRequest::PlayCompositeWaveform { composite_waveform, responder } => {
                let duration = composite_waveform
                    .iter()
                    .map(|efffect| {
                        let delay = Duration::from_nanos(
                            efffect.delay.try_into().context("Failed to convert to u64")?,
                        );
                        let duration = match efffect.waveform {
                            CompositeEffectWaveform::Noop => Duration::default(),
                            _ => COMPOSITE_EFFECT_DURATION,
                        };
                        Ok(delay + duration)
                    })
                    .sum::<anyhow::Result<Duration>>()
                    .context("Failed to get composite waveform duration")?;
                Timer::new(duration).await;
                responder.send(Ok(())).context("Failed to send response")?;
            }
            DeviceRequest::StopVibration { responder } => {
                responder.send(Ok(())).context("Failed to send response")?;
            }
            DeviceRequest::SetAmplitude { amplitude, responder } => {
                if amplitude <= 0.0 || amplitude > 1.0 {
                    responder
                        .send(Err(zx::Status::INVALID_ARGS.into_raw()))
                        .context("Failed to send response")?;
                    continue;
                }

                responder.send(Ok(())).context("Failed to send response")?;
            }
            DeviceRequest::GetProperties { responder } => {
                // Value must be greater than 0 in order to pass tests.
                const FUNDAMENTAL_RESONANT_FREQUENCY_HZ: f32 = 123.0;

                // Value must be greater than 0 in order to pass tests.
                const QUALITY_FACTOR: f32 = 789.0;

                const SUPPORTED_EFFECTS: [SupportedEffect; 22] = [
                    SupportedEffect { effect: Effect::Click, duration: 0 },
                    SupportedEffect { effect: Effect::DoubleClick, duration: 0 },
                    SupportedEffect { effect: Effect::Tick, duration: 0 },
                    SupportedEffect { effect: Effect::Thud, duration: 0 },
                    SupportedEffect { effect: Effect::Pop, duration: 0 },
                    SupportedEffect { effect: Effect::HeavyClick, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone1, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone2, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone3, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone4, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone5, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone6, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone7, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone8, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone9, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone10, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone11, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone12, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone13, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone14, duration: 0 },
                    SupportedEffect { effect: Effect::Ringtone15, duration: 0 },
                    SupportedEffect { effect: Effect::TextureTick, duration: 0 },
                ];

                let composite_effect_duration_ns: i64 = COMPOSITE_EFFECT_DURATION
                    .as_nanos()
                    .try_into()
                    .context("Failed to conver to i64")?;

                // The specified items are required in order to pass tests.
                let supported_composite_effect_waveforms: [SupportedCompositeEffectWaveform; 9] = [
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::Noop,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::Click,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::Thud,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::Spin,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::QuickRise,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::SlowRise,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::QuickFall,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::LightTick,
                        duration: composite_effect_duration_ns,
                    },
                    SupportedCompositeEffectWaveform {
                        waveform: CompositeEffectWaveform::LowTick,
                        duration: composite_effect_duration_ns,
                    },
                ];

                // Value must be greater than 100ms in order to execute all related tests.
                const MAX_COMPOSITE_EFFECT_DELAY: Duration = Duration::from_millis(100);

                // Value must be greater than 0 in order to pass tests.
                // Value must be greater than 5 in order to execute all related tests.
                const MAX_COMPOSITE_EFFECT_COUNT: u64 = 5;

                let max_composite_effect_delay = MAX_COMPOSITE_EFFECT_DELAY
                    .as_nanos()
                    .try_into()
                    .context("Failed to conver to i64")?;
                responder
                    .send(Ok((
                        FUNDAMENTAL_RESONANT_FREQUENCY_HZ,
                        QUALITY_FACTOR,
                        &SUPPORTED_EFFECTS,
                        &supported_composite_effect_waveforms,
                        MAX_COMPOSITE_EFFECT_COUNT,
                        max_composite_effect_delay,
                    )))
                    .context("Failed to send response")?;
            }
            DeviceRequest::_UnknownMethod { ordinal, .. } => {
                error!("Received unknown method {}", ordinal);
            }
        };
    }
    Ok(())
}

enum IncomingService {
    Haptics(ServiceRequest),
}

#[fuchsia::main]
async fn main() -> anyhow::Result<()> {
    // Initialize the outgoing services provided by this component
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service_instance("default", IncomingService::Haptics);
    fs.take_and_serve_directory_handle()?;

    fs.for_each_concurrent(1, |IncomingService::Haptics(ServiceRequest::Device(device))| {
        handle_device_requests(device).unwrap_or_else(|e| println!("{:?}", e))
    })
    .await;

    Ok(())
}
