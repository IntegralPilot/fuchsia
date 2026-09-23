// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use derivative::Derivative;
use fidl_fuchsia_hardware_audio::*;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::{Future, StreamExt};
use log::warn;
use std::sync::Arc;
use vfs::directory::entry_container::Directory;
use vfs::{pseudo_directory, service};

use crate::DigitalAudioInterface;
use crate::driver::{ensure_dai_format_is_supported, ensure_pcm_format_is_supported};

/// The status of the current device.  Retrievable via `TestHandle::status`.
#[derive(Derivative, Clone, Debug)]
#[derivative(Default)]
pub enum TestStatus {
    #[derivative(Default)]
    Idle,
    Configured {
        dai_format: DaiFormat,
        pcm_format: PcmFormat,
    },
    Started {
        dai_format: DaiFormat,
        pcm_format: PcmFormat,
    },
}

impl TestStatus {
    fn start(&mut self) -> Result<(), ()> {
        if let Self::Configured { dai_format, pcm_format } = self {
            *self = Self::Started { dai_format: *dai_format, pcm_format: *pcm_format };
            Ok(())
        } else {
            Err(())
        }
    }

    fn stop(&mut self) {
        if let Self::Started { dai_format, pcm_format } = *self {
            *self = Self::Configured { dai_format, pcm_format };
        }
    }
}

#[derive(Clone)]
pub struct TestHandle(Arc<Mutex<TestStatus>>);

impl TestHandle {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(TestStatus::default())))
    }

    pub fn status(&self) -> TestStatus {
        self.0.lock().clone()
    }

    pub fn is_started(&self) -> bool {
        let lock = self.0.lock();
        match *lock {
            TestStatus::Started { .. } => true,
            _ => false,
        }
    }

    fn set_configured(&self, dai_format: DaiFormat, pcm_format: PcmFormat) {
        let mut lock = self.0.lock();
        *lock = TestStatus::Configured { dai_format, pcm_format };
    }

    fn start(&self) -> Result<(), ()> {
        self.0.lock().start()
    }

    fn stop(&self) {
        self.0.lock().stop()
    }
}

/// Logs and breaks out of the loop if the result is an Error.
macro_rules! log_error {
    ($result:expr, $tag:expr) => {
        if let Err(e) = $result {
            warn!("Error sending {}: {:?}", $tag, e);
            break;
        }
    };
}

async fn handle_ring_buffer(mut requests: RingBufferRequestStream, handle: TestHandle) {
    while let Some(req) = requests.next().await {
        if let Err(e) = req {
            warn!("Error processing RingBuffer request stream: {:?}", e);
            break;
        }
        match req.unwrap() {
            RingBufferRequest::Start { responder } => match handle.start() {
                Ok(()) => log_error!(responder.send(0), "ring buffer start response"),
                Err(()) => {
                    warn!("Started when we couldn't expect it, shutting down");
                }
            },
            RingBufferRequest::Stop { responder } => {
                handle.stop();
                log_error!(responder.send(), "ring buffer stop response");
            }
            x => unimplemented!("RingBuffer Request not implemented: {:?}", x),
        };
    }
}

async fn test_handle_dai_requests(
    dai_formats: DaiSupportedFormats,
    pcm_formats: SupportedFormats,
    as_input: bool,
    mut requests: DaiRequestStream,
    handle: TestHandle,
) {
    use std::slice::from_ref;
    let properties = DaiProperties {
        is_input: Some(as_input),
        manufacturer: Some("Fuchsia".to_string()),
        ..Default::default()
    };

    #[allow(clippy::collection_is_never_read)]
    let mut _rb_task = None;
    while let Some(req) = requests.next().await {
        if let Err(e) = req {
            warn!("Error processing DAI request stream: {:?}", e);
            break;
        }
        match req.unwrap() {
            DaiRequest::GetProperties { responder } => {
                log_error!(responder.send(&properties), "properties response");
            }
            DaiRequest::GetDaiFormats { responder } => {
                log_error!(responder.send(Ok(from_ref(&dai_formats))), "formats response");
            }
            DaiRequest::GetRingBufferFormats { responder } => {
                log_error!(responder.send(Ok(from_ref(&pcm_formats))), "pcm response");
            }
            DaiRequest::CreateRingBuffer {
                dai_format, ring_buffer_format, ring_buffer, ..
            } => {
                let shutdown_bad_args =
                    |server: fidl::endpoints::ServerEnd<RingBufferMarker>, err: anyhow::Error| {
                        warn!("CreateRingBuffer: {:?}", err);
                        if let Err(e) = server.close_with_epitaph(zx::Status::INVALID_ARGS) {
                            warn!("Couldn't send ring buffer epitaph: {:?}", e);
                        }
                    };
                if let Err(e) = ensure_dai_format_is_supported(from_ref(&dai_formats), &dai_format)
                {
                    shutdown_bad_args(ring_buffer, e);
                    continue;
                }
                let pcm_format = match ring_buffer_format.pcm_format {
                    Some(f) => f,
                    None => {
                        shutdown_bad_args(ring_buffer, format_err!("Only PCM format supported"));
                        continue;
                    }
                };
                if let Err(e) = ensure_pcm_format_is_supported(from_ref(&pcm_formats), &pcm_format)
                {
                    shutdown_bad_args(ring_buffer, e);
                    continue;
                }
                handle.set_configured(dai_format, pcm_format);
                let requests = ring_buffer.into_stream();
                _rb_task = Some(fasync::Task::spawn(handle_ring_buffer(requests, handle.clone())));
            }
            x => unimplemented!("DAI request not implemented: {:?}", x),
        };
    }
}

/// Represents a mock DAI device that processes `Dai` requests from the provided `requests`
/// stream.
/// Returns a Future representing the processing task and a `TestHandle` which can be used
/// to validate behavior.
fn mock_dai_device(
    as_input: bool,
    requests: DaiRequestStream,
) -> (impl Future<Output = ()>, TestHandle) {
    let supported_dai_formats = DaiSupportedFormats {
        number_of_channels: vec![1],
        sample_formats: vec![DaiSampleFormat::PcmSigned],
        frame_formats: vec![DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::Tdm1)],
        frame_rates: vec![8000, 16000, 32000, 48000, 96000],
        bits_per_slot: vec![16],
        bits_per_sample: vec![16],
    };

    let number_of_channels = 1usize;
    let attributes = vec![ChannelAttributes::default(); number_of_channels];
    let channel_set = ChannelSet { attributes: Some(attributes), ..Default::default() };
    let supported_pcm_formats = SupportedFormats {
        pcm_supported_formats: Some(PcmSupportedFormats {
            channel_sets: Some(vec![channel_set]),
            sample_formats: Some(vec![SampleFormat::PcmSigned]),
            bytes_per_sample: Some(vec![2]),
            valid_bits_per_sample: Some(vec![16]),
            frame_rates: Some(vec![8000, 16000, 32000, 48000, 96000]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let handle = TestHandle::new();

    let handler_fut = test_handle_dai_requests(
        supported_dai_formats,
        supported_pcm_formats,
        as_input,
        requests,
        handle.clone(),
    );
    (handler_fut, handle)
}

/// Builds and returns a DigitalAudioInterface for testing scenarios. Returns the
/// `TestHandle` associated with this device which can be used to validate behavior.
pub fn test_digital_audio_interface(as_input: bool) -> (DigitalAudioInterface, TestHandle) {
    let (proxy, requests) = fidl::endpoints::create_proxy_and_stream::<DaiMarker>();

    let (handler, handle) = mock_dai_device(as_input, requests);
    fasync::Task::spawn(handler).detach();

    (DigitalAudioInterface::from_proxy(proxy), handle)
}

async fn handle_dai_connect_requests(as_input: bool, mut stream: DaiConnectorRequestStream) {
    while let Some(request) = stream.next().await {
        if let Ok(DaiConnectorRequest::Connect { dai_protocol, .. }) = request {
            let (handler, _test_handle) = mock_dai_device(as_input, dai_protocol.into_stream());
            fasync::Task::spawn(handler).detach();
        }
    }
}

/// Builds and returns a VFS with a mock input and output DAI service.
pub fn mock_dai_service_with_io_devices(
    input: &'static str,
    output: &'static str,
) -> Arc<dyn Directory> {
    pseudo_directory! {
        "fuchsia.hardware.audio.DaiConnectorService" => pseudo_directory! {
            input => pseudo_directory! {
                "dai_connector" => service::host(
                    move |stream: DaiConnectorRequestStream| handle_dai_connect_requests(true,
                                                                                         stream)
                ),
            },
            output => pseudo_directory! {
                "dai_connector" => service::host(
                    move |stream: DaiConnectorRequestStream| handle_dai_connect_requests(false,
                                                                                         stream)
                ),
            },
        }
    }
}
