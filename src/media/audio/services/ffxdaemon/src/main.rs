// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod capturer;
mod clock;
mod device;
mod error;
mod renderer;
mod ring_buffer;
mod wav_socket;

use capturer::Capturer;
use device::{Device, DeviceControlConnector};
use renderer::Renderer;
use wav_socket::WavSocket;

use anyhow::{Context, Error, anyhow};
use error::ControllerError;
use fidl_fuchsia_audio_controller as fac;
use fidl_fuchsia_media as fmedia;
use fuchsia_async as fasync;
use fuchsia_audio::Format;
use fuchsia_audio::device::Selector;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use futures::lock::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::error;
use std::sync::Arc;
use std::time::Duration;

/// Wraps all hosted protocols into a single type that can be matched against and dispatched.
enum IncomingRequest {
    DeviceControl(fac::DeviceControlRequestStream),
    Player(fac::PlayerRequestStream),
    Recorder(fac::RecorderRequestStream),
}

pub async fn handle_play_request(
    device_control_connector: Arc<Mutex<DeviceControlConnector>>,
    request: fac::PlayerPlayRequest,
) -> Result<fac::PlayerPlayResponse, ControllerError> {
    let destination = request.destination.ok_or_else(|| {
        ControllerError::new(fac::Error::ArgumentsMissing, format!("destination missing"))
    })?;

    if let fac::PlayDestination::Renderer(fac::RendererConfig::StandardRenderer(ref std_ren)) =
        destination
    {
        if let Some(ref usage) = std_ren.usage {
            if usage.into_primitive() >= fmedia::RENDER_USAGE2_COUNT as u32 {
                return Err(ControllerError::new(
                    fac::Error::InvalidArguments,
                    format!("Invalid AudioRenderUsage2: usage is >= RENDER_USAGE2_COUNT"),
                ));
            }
        }
    }

    let mut wav_socket = {
        let wav_source = request.wav_source.ok_or_else(|| {
            ControllerError::new(fac::Error::ArgumentsMissing, format!("wav_source missing"))
        })?;
        WavSocket(fasync::Socket::from_socket(wav_source))
    };

    match destination {
        fac::PlayDestination::Renderer(config) => {
            let spec = wav_socket.read_header().await?;
            let format = Format::from(spec);

            let renderer =
                Renderer::new(config, format, request.gain_settings).await.map_err(|err| {
                    ControllerError::new(
                        fac::Error::DeviceNotReachable,
                        format!("Failed to connect to renderer: {err}"),
                    )
                })?;

            renderer.play(wav_socket).await
        }
        fac::PlayDestination::DeviceRingBuffer(fac::DeviceRingBuffer {
            selector: fidl_selector,
            ring_buffer_element_id,
        }) => {
            let selector = Selector::try_from(fidl_selector).map_err(|msg| {
                ControllerError::new(
                    fac::Error::InvalidArguments,
                    format!("invalid selector: {msg}"),
                )
            })?;

            let device_control = device_control_connector.lock().await.connect(selector).await?;
            let mut device = Device::new(device_control);

            device.play(ring_buffer_element_id, wav_socket, request.active_channels_bitmask).await
        }
        fac::PlayDestinationUnknown!() => Err(ControllerError::new(
            fac::Error::InvalidArguments,
            format!("Unknown PlayDestination"),
        )),
    }
}

async fn serve_player(
    device_control_connector: Arc<Mutex<DeviceControlConnector>>,
    mut stream: fac::PlayerRequestStream,
) -> Result<(), Error> {
    while let Ok(Some(request)) = stream.try_next().await {
        let request_name = request.method_name();
        match request {
            fac::PlayerRequest::Play { payload, responder } => {
                let result = handle_play_request(device_control_connector.clone(), payload).await;

                if let Err(ref err) = result {
                    error!(err:%; "Failed to play");
                }
                responder.send(result.map_err(|err| err.inner)).context("Could not send response")
            }
            _ => Err(anyhow!("Unknown request {request_name}")),
        }?;
    }
    Ok(())
}

pub async fn handle_record_request(
    device_control_connector: Arc<Mutex<DeviceControlConnector>>,
    request: fac::RecorderRecordRequest,
) -> Result<fac::RecorderRecordResponse, ControllerError> {
    let source = request.source.ok_or_else(|| {
        ControllerError::new(fac::Error::ArgumentsMissing, format!("source missing"))
    })?;
    if let fac::RecordSource::Capturer(fac::CapturerConfig::StandardCapturer(ref std_cap)) = source
    {
        if let Some(ref usage) = std_cap.usage {
            if usage.into_primitive() >= fmedia::CAPTURE_USAGE2_COUNT as u32 {
                return Err(ControllerError::new(
                    fac::Error::InvalidArguments,
                    format!("Invalid CaptureUsage2: usage is >= CAPTURE_USAGE2_COUNT"),
                ));
            }
        }
    }
    let stream_type = request.stream_type.ok_or_else(|| {
        ControllerError::new(fac::Error::ArgumentsMissing, format!("stream_type missing"))
    })?;
    let wav_socket = {
        let wav_data = request.wav_data.ok_or_else(|| {
            ControllerError::new(fac::Error::ArgumentsMissing, format!("wav_data missing"))
        })?;
        WavSocket(fasync::Socket::from_socket(wav_data))
    };

    let duration = request.duration.map(|duration| Duration::from_nanos(duration as u64));
    let format = Format::from(stream_type);

    match source {
        fac::RecordSource::Capturer(..) | fac::RecordSource::Loopback(..) => {
            let mut capturer =
                Capturer::new(source, format, request.gain_settings).await.map_err(|err| {
                    ControllerError::new(
                        fac::Error::DeviceNotReachable,
                        format!("Failed to connect to capturer: {err}"),
                    )
                })?;

            capturer.record(wav_socket, duration, request.canceler, request.buffer_size).await
        }
        fac::RecordSource::DeviceRingBuffer(fac::DeviceRingBuffer {
            selector: fidl_selector,
            ring_buffer_element_id,
        }) => {
            let selector = Selector::try_from(fidl_selector).map_err(|msg| {
                ControllerError::new(
                    fac::Error::InvalidArguments,
                    format!("invalid selector: {msg}"),
                )
            })?;

            let device_control = device_control_connector.lock().await.connect(selector).await?;
            let mut device = Device::new(device_control);

            device
                .record(ring_buffer_element_id, format, wav_socket, duration, request.canceler)
                .await
        }
        fac::RecordSourceUnknown!() => {
            Err(ControllerError::new(fac::Error::InvalidArguments, format!("Unknown RecordSource")))
        }
    }
}

async fn serve_recorder(
    device_control_connector: Arc<Mutex<DeviceControlConnector>>,
    mut stream: fac::RecorderRequestStream,
) -> Result<(), Error> {
    while let Ok(Some(request)) = stream.try_next().await {
        let request_name = request.method_name();
        match request {
            fac::RecorderRequest::Record { payload, responder } => {
                let result = handle_record_request(device_control_connector.clone(), payload).await;

                if let Err(ref err) = result {
                    error!(err:%; "Failed to record");
                }
                responder.send(result.map_err(|err| err.inner)).context("Could not send response")
            }
            _ => Err(anyhow!("Unknown request {request_name}")),
        }?;
    }
    Ok(())
}

// TODO(b/298683668) this will be removed, replaced by client direct calls.
async fn serve_device_control(
    device_control_connector: Arc<Mutex<DeviceControlConnector>>,
    mut stream: fac::DeviceControlRequestStream,
) -> Result<(), Error> {
    while let Ok(Some(request)) = stream.try_next().await {
        let request_name = request.method_name();
        let request_result = match request {
            fac::DeviceControlRequest::DeviceSetGainState { payload, responder } => {
                let selector = payload
                    .device
                    .ok_or_else(|| anyhow!("No device specified"))?
                    .try_into()
                    .map_err(|msg| anyhow!("invalid selector: {msg}"))?;
                let gain_state =
                    payload.gain_state.ok_or_else(|| anyhow!("No gain state specified"))?;

                let device_control =
                    device_control_connector.lock().await.connect(selector).await?;
                let mut device = Device::new(device_control);

                device.set_gain(gain_state).await?;
                responder.send(Ok(())).map_err(|e| anyhow!("Error sending response: {e}"))
            }
            _ => Err(anyhow!("Request {request_name} not supported.")),
        };
        match request_result {
            Ok(_) => println!("Request succeeded."),
            Err(e) => {
                let error_msg = format!("Request {request_name} failed with error {e} \n");
                println!("{}", error_msg);
            }
        }
    }
    Ok(())
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), Error> {
    // Register a trace provider
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let mut service_fs = ServiceFs::new_local();

    // Initialize inspect
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    component::health().set_starting_up();

    // Add services here. E.g:
    service_fs.dir("svc").add_fidl_service(IncomingRequest::DeviceControl);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Player);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Recorder);
    service_fs.take_and_serve_directory_handle().context("Failed to serve outgoing namespace")?;

    component::health().set_ok();

    let device_control_connector = Arc::new(Mutex::new(DeviceControlConnector::new()));

    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| {
            let device_control_connector = device_control_connector.clone();
            async {
                match request {
                    IncomingRequest::DeviceControl(stream) => {
                        if let Err(err) =
                            serve_device_control(device_control_connector, stream).await
                        {
                            error!(err:%; "Failed to serve DeviceControl protocol");
                        }
                    }
                    IncomingRequest::Player(stream) => {
                        if let Err(err) = serve_player(device_control_connector, stream).await {
                            error!(err:%; "Failed to serve Player protocol");
                        }
                    }
                    IncomingRequest::Recorder(stream) => {
                        if let Err(err) = serve_recorder(device_control_connector, stream).await {
                            error!(err:%; "Failed to serve Recorder protocol");
                        }
                    }
                }
            }
        })
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_play_unknown_render_usage() {
        let connector = Arc::new(Mutex::new(DeviceControlConnector::new()));
        let request = fac::PlayerPlayRequest {
            wav_source: Some(fidl::Socket::create_stream().0),
            destination: Some(fac::PlayDestination::Renderer(
                fac::RendererConfig::StandardRenderer(fac::StandardRendererConfig {
                    usage: Some(fmedia::AudioRenderUsage2::from_primitive_allow_unknown(42)),
                    ..Default::default()
                }),
            )),
            ..Default::default()
        };
        let res = handle_play_request(connector, request).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_record_unknown_capture_usage() {
        let connector = Arc::new(Mutex::new(DeviceControlConnector::new()));
        let request = fac::RecorderRecordRequest {
            wav_data: Some(fidl::Socket::create_stream().0),
            source: Some(fac::RecordSource::Capturer(fac::CapturerConfig::StandardCapturer(
                fac::StandardCapturerConfig {
                    usage: Some(fmedia::AudioCaptureUsage2::from_primitive_allow_unknown(68)),
                    ..Default::default()
                },
            ))),
            stream_type: Some(fmedia::AudioStreamType {
                sample_format: fmedia::AudioSampleFormat::Signed16,
                channels: 1,
                frames_per_second: 8000,
            }),
            ..Default::default()
        };
        let res = handle_record_request(connector, request).await;
        assert!(res.is_err());
    }
}
