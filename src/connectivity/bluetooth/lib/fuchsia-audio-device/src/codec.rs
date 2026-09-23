// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::hanging_get::server as hanging_server;
use fidl::endpoints::{ClientEnd, Responder};
use fidl_fuchsia_hardware_audio::{
    CodecMarker, CodecRequestStream, CodecStartResponder, CodecStopResponder,
    CodecWatchPlugStateResponder, PlugState,
};
use fidl_fuchsia_hardware_audio_signalprocessing::SignalProcessingRequestStream;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::stream::FusedStream;
use futures::task::{Context, Poll};
use futures::{Stream, StreamExt};
use std::fmt::Debug;
use std::sync::Arc;

use crate::types::{Error, Result};

/// A software fuchsia audio codec.
/// Codecs communicate their capabilities to the system and provide control points for components
/// to start, stop, and configure media capabilities.
/// See [[Audio Codec Interface]](https://fuchsia.dev/fuchsia-src/concepts/drivers/driver_architectures/audio_drivers/audio_codec)
pub struct SoftCodec {
    /// Requests from Media for Codec control (start/stop)
    codec_requests: Option<CodecRequestStream>,
    /// Requests from Media if the SignalProcessing Connector is connected
    signal_processing_requests: Option<SignalProcessingRequestStream>,
    /// CodecProperties of this device (do not change)
    properties: fidl_fuchsia_hardware_audio::CodecProperties,
    /// Formats that are supported by this codec.  These do not change for the lifetime of the
    /// codec.
    supported_formats: [fidl_fuchsia_hardware_audio::DaiSupportedFormats; 1],
    /// Selected format, if it is selected
    selected_format: Arc<Mutex<Option<fidl_fuchsia_hardware_audio::DaiFormat>>>,
    /// Codec Start state, including time started/stopped.
    /// Used to correctly respond to Start / Stop when already started/stopped
    start_state: Arc<Mutex<StartState>>,
    /// Plug State publisher
    plug_state_publisher: hanging_server::Publisher<
        PlugState,
        CodecWatchPlugStateResponder,
        Box<dyn Fn(&PlugState, CodecWatchPlugStateResponder) -> bool>,
    >,
    /// Plug State subscriber
    plug_state_subscriber: hanging_server::Subscriber<
        PlugState,
        CodecWatchPlugStateResponder,
        Box<dyn Fn(&PlugState, CodecWatchPlugStateResponder) -> bool>,
    >,
    /// True when the codec has terminated, and should not be polled any more.
    terminated: TerminatedState,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum TerminatedState {
    #[default]
    NotTerminated,
    /// Terminating. We have returned an Err, but not a None yet.
    Terminating,
    /// We have returned a None, no one should poll us again.
    Terminated,
}

#[derive(Debug)]
pub enum StartState {
    Stopped(zx::MonotonicInstant),
    Starting(Option<CodecStartResponder>),
    Started(zx::MonotonicInstant),
    Stopping(Option<CodecStopResponder>),
}

impl Default for StartState {
    fn default() -> Self {
        Self::Stopped(fasync::MonotonicInstant::now().into())
    }
}

pub enum CodecDirection {
    Input,
    Output,
    Duplex,
}

/// Request from the Audio subsystem to query or perform an action on the codec.
pub enum CodecRequest {
    /// Set the format of this codec.
    SetFormat {
        /// The format requested.
        format: fidl_fuchsia_hardware_audio::DaiFormat,
        /// Responder to be called when the format has been set with Ok(()) or Err(Status) if
        /// setting the format failed.
        responder: Box<dyn FnOnce(std::result::Result<(), zx::Status>) + Send>,
    },
    /// Start the codec.
    Start {
        /// Responder to be called when the codec has been started, providing the time started or
        /// an error if it failed to start.
        /// The time is system monotonic when configuring the codec to start completed (it does not
        /// include startup delay time)
        /// Replying Err() to this will close the codec, and it will need to be re-instantiated.
        responder: Box<dyn FnOnce(std::result::Result<zx::MonotonicInstant, zx::Status>) + Send>,
    },
    /// Stop the codec.
    Stop {
        /// Responder to be called when the codec has been configured to stop.
        /// On success, provides the time that the codec was configured, not including delays.
        /// On failuyre, this will close the codec and it will need to be re-instantiated.
        responder: Box<dyn FnOnce(std::result::Result<zx::MonotonicInstant, zx::Status>) + Send>,
    },
}

impl Debug for CodecRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetFormat { format, .. } => {
                f.debug_struct("SetFormat").field("format", format).finish()
            }
            Self::Start { .. } => f.debug_struct("Start").finish(),
            Self::Stop { .. } => f.debug_struct("Stop").finish(),
        }
    }
}

impl CodecRequest {
    fn start(state: Arc<Mutex<StartState>>) -> Self {
        Self::Start {
            responder: Box::new(move |time| {
                let mut lock = state.lock();
                let StartState::Starting(ref mut responder) = *lock else {
                    log::warn!("Not in starting state, noop response");
                    return;
                };
                let Some(responder) = responder.take() else {
                    log::warn!("Responder missing for start, noop");
                    return;
                };
                let Ok(time) = time else {
                    // Some error happened here, we close the channel by dropping the responder.
                    drop(responder);
                    return;
                };
                *lock = StartState::Started(time);
                let _ = responder.send(time.into_nanos());
            }),
        }
    }

    fn stop(state: Arc<Mutex<StartState>>) -> Self {
        Self::Stop {
            responder: Box::new(move |time| {
                let mut lock = state.lock();
                let StartState::Stopping(ref mut responder) = *lock else {
                    log::warn!("Not in stopping state, noop response");
                    return;
                };
                let Some(responder) = responder.take() else {
                    log::warn!("Responder missing for stop, noop");
                    return;
                };
                let Ok(time) = time else {
                    // Some error happened here, we close the channel by dropping the responder.
                    drop(responder);
                    return;
                };
                *lock = StartState::Stopped(time);
                let _ = responder.send(time.into_nanos());
            }),
        }
    }
}

impl SoftCodec {
    pub fn create(
        unique_id: Option<&[u8; 16]>,
        manufacturer: &str,
        product: &str,
        direction: CodecDirection,
        formats: fidl_fuchsia_hardware_audio::DaiSupportedFormats,
        initially_plugged: bool,
    ) -> (Self, ClientEnd<CodecMarker>) {
        let (client, codec_requests) = fidl::endpoints::create_request_stream::<CodecMarker>();
        let is_input = match direction {
            CodecDirection::Input => Some(true),
            CodecDirection::Output => Some(false),
            CodecDirection::Duplex => None,
        };
        let properties = fidl_fuchsia_hardware_audio::CodecProperties {
            is_input,
            manufacturer: Some(manufacturer.to_string()),
            product: Some(product.to_string()),
            unique_id: unique_id.cloned(),
            plug_detect_capabilities: Some(
                fidl_fuchsia_hardware_audio::PlugDetectCapabilities::CanAsyncNotify,
            ),
            ..Default::default()
        };
        let plug_state = fidl_fuchsia_hardware_audio::PlugState {
            plugged: Some(initially_plugged),
            plug_state_time: Some(fuchsia_async::MonotonicInstant::now().into_nanos()),
            ..Default::default()
        };
        let mut plug_state_server = hanging_server::HangingGet::<
            _,
            _,
            Box<dyn Fn(&PlugState, CodecWatchPlugStateResponder) -> bool>,
        >::new(
            plug_state,
            Box::new(|state, responder: CodecWatchPlugStateResponder| {
                let _ = responder.send(state);
                true
            }),
        );
        let plug_state_publisher = plug_state_server.new_publisher();
        let plug_state_subscriber = plug_state_server.new_subscriber();
        (
            Self {
                codec_requests: Some(codec_requests),
                signal_processing_requests: Default::default(),
                properties,
                supported_formats: [formats],
                selected_format: Default::default(),
                start_state: Default::default(),
                plug_state_publisher,
                plug_state_subscriber,
                terminated: Default::default(),
            },
            client,
        )
    }

    pub fn update_plug_state(&self, plugged: bool) -> Result<()> {
        self.plug_state_publisher.set(fidl_fuchsia_hardware_audio::PlugState {
            plugged: Some(plugged),
            plug_state_time: Some(fasync::MonotonicInstant::now().into_nanos()),
            ..Default::default()
        });
        Ok(())
    }

    /// Polls the Codec requests, and handles any requests it can, or returns Poll::Ready if the
    /// request requires a response, generates an event, or an error occurs.
    fn poll_codec(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<CodecRequest>>> {
        let Some(codec_requests) = self.codec_requests.as_mut() else {
            panic!("Codec requests polled without a request stream");
        };
        loop {
            let request = futures::ready!(codec_requests.poll_next_unpin(cx));
            let request = match request {
                None => {
                    self.terminated = TerminatedState::Terminating;
                    return Poll::Ready(Some(Err(Error::RequestStreamError(
                        fidl::Error::ClientRead(zx::Status::PEER_CLOSED.into()),
                    ))));
                }
                Some(Err(e)) => {
                    self.terminated = TerminatedState::Terminating;
                    return Poll::Ready(Some(Err(Error::RequestStreamError(e))));
                }
                Some(Ok(request)) => request,
            };
            use fidl_fuchsia_hardware_audio::CodecRequest::*;
            log::info!("Handling codec request: {request:?}");
            match request {
                GetHealthState { responder } => {
                    let _ = responder.send(&fidl_fuchsia_hardware_audio::HealthState::default());
                }
                SignalProcessingConnect { protocol, .. } => {
                    if self.signal_processing_requests.is_some() {
                        let _ = protocol.close_with_epitaph(zx::Status::ALREADY_BOUND);
                        continue;
                    }
                    // TODO(https://fxbug.dev/323576893): Support the Signal Processing connect.
                    let _ = protocol.close_with_epitaph(zx::Status::NOT_SUPPORTED);
                }
                Reset { responder } => {
                    // TODO(https://fxbug.dev/324247020): Support resetting.
                    responder.control_handle().shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                    continue;
                }
                GetProperties { responder } => {
                    let _ = responder.send(&self.properties);
                }
                Start { responder } => {
                    let mut lock = self.start_state.lock();
                    match *lock {
                        StartState::Started(time) => {
                            let _ = responder.send(time.into_nanos());
                            continue;
                        }
                        StartState::Stopped(_) => {
                            *lock = StartState::Starting(Some(responder));
                            return Poll::Ready(Some(Ok(CodecRequest::start(
                                self.start_state.clone(),
                            ))));
                        }
                        StartState::Starting(ref mut existing_responder) => {
                            // Started while starting
                            log::warn!("Got start while starting, closing codec");
                            drop(responder);
                            drop(existing_responder.take());
                        }
                        _ => {
                            // Started while stopping
                            log::warn!("Got start while stopping, closing codec");
                            drop(responder);
                        }
                    }
                }
                Stop { responder } => {
                    let mut lock = self.start_state.lock();
                    match *lock {
                        StartState::Stopped(time) => {
                            let _ = responder.send(time.into_nanos());
                            continue;
                        }
                        StartState::Started(_) => {
                            *lock = StartState::Stopping(Some(responder));
                            return Poll::Ready(Some(Ok(CodecRequest::stop(
                                self.start_state.clone(),
                            ))));
                        }
                        StartState::Stopping(ref mut existing_responder) => {
                            // Stopped while stopping
                            log::warn!("Got stop while stopping, closing codec");
                            drop(responder);
                            drop(existing_responder.take());
                        }
                        _ => {
                            // Started while stopping.
                            log::warn!("Got stop while starting, closing codec");
                            drop(responder);
                        }
                    }
                }
                GetDaiFormats { responder } => {
                    let _ = responder.send(Ok(self.supported_formats.as_slice()));
                }
                SetDaiFormat { format, responder } => {
                    let responder = Box::new({
                        let selected = self.selected_format.clone();
                        let format = format.clone();
                        move |result: std::result::Result<(), zx::Status>| {
                            let _ = match result {
                                // TODO(https://fxbug.dev/42128949): support specifying delay times
                                Ok(()) => {
                                    *selected.lock() = Some(format);
                                    responder.send(Ok(
                                        &fidl_fuchsia_hardware_audio::CodecFormatInfo::default(),
                                    ))
                                }
                                Err(s) => responder.send(Err(s.into_raw())),
                            };
                        }
                    });
                    return Poll::Ready(Some(Ok(CodecRequest::SetFormat { format, responder })));
                }
                WatchPlugState { responder } => {
                    if let Err(_e) = self.plug_state_subscriber.register(responder) {
                        // Responder will be dropped, closing the protocol.
                        self.terminated = TerminatedState::Terminating;
                        return Poll::Ready(Some(Err(Error::PeerError(
                            "WatchPlugState while hanging".to_string(),
                        ))));
                    }
                }
            }
        }
    }

    /// Polls the SignalProcessing requests if it exists, and handles any requests it can, or returns Poll::Ready
    /// if the request requires a response, generates an event, or an error occurs.
    fn poll_signal(&mut self, _cx: &mut Context<'_>) -> Poll<Option<Result<CodecRequest>>> {
        let Some(_requests) = self.signal_processing_requests.as_mut() else {
            // The codec will wake the context when SignalProcessing is connected.
            return Poll::Pending;
        };
        // TODO(https://fxbug.dev/323576893): Support Signal Processing
        Poll::Pending
    }
}

impl FusedStream for SoftCodec {
    fn is_terminated(&self) -> bool {
        self.terminated == TerminatedState::Terminated
    }
}

impl Stream for SoftCodec {
    type Item = Result<CodecRequest>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.terminated {
            TerminatedState::Terminating => {
                self.terminated = TerminatedState::Terminated;
                self.codec_requests = None;
                return Poll::Ready(None);
            }
            TerminatedState::Terminated => panic!("polled while terminated"),
            TerminatedState::NotTerminated => {}
        };
        if let Poll::Ready(x) = self.poll_codec(cx) {
            return Poll::Ready(x);
        }
        self.poll_signal(cx)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use async_utils::PollExt;
    use fixture::fixture;

    use fidl_fuchsia_hardware_audio::{
        CodecProxy, DaiFrameFormat, DaiFrameFormatStandard, DaiSampleFormat, DaiSupportedFormats,
    };

    const TEST_UNIQUE_ID: [u8; 16] = [5; 16];
    const TEST_MANUF: &'static str = "Fuchsia";
    const TEST_PRODUCT: &'static str = "Test Codec";

    pub(crate) fn with_soft_codec<F>(_name: &str, test: F)
    where
        F: FnOnce(fasync::TestExecutor, CodecProxy, SoftCodec) -> (),
    {
        let exec = fasync::TestExecutor::new();
        let (codec, client) = SoftCodec::create(
            Some(&TEST_UNIQUE_ID),
            TEST_MANUF,
            TEST_PRODUCT,
            CodecDirection::Output,
            DaiSupportedFormats {
                number_of_channels: vec![1, 2],
                sample_formats: vec![DaiSampleFormat::PcmUnsigned],
                // TODO(https://fxbug.dev/278283913): Configuration for which FrameFormatStandard the chip expects
                frame_formats: vec![DaiFrameFormat::FrameFormatStandard(
                    DaiFrameFormatStandard::I2S,
                )],
                frame_rates: vec![48000],
                bits_per_slot: vec![16],
                bits_per_sample: vec![16],
            },
            true,
        );
        test(exec, client.into_proxy(), codec)
    }

    #[fixture(with_soft_codec)]
    #[fuchsia::test]
    fn happy_lifecycle(mut exec: fasync::TestExecutor, proxy: CodecProxy, mut codec: SoftCodec) {
        let mut codec_next_fut = codec.next();
        let mut get_properties_fut = proxy.get_properties();
        // Codec can answer this on it's own, this should not complete.
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");
        let properties =
            exec.run_until_stalled(&mut get_properties_fut).expect("finish").expect("ok");

        assert_eq!(properties.unique_id, Some(TEST_UNIQUE_ID));
        assert_eq!(
            properties.plug_detect_capabilities,
            Some(fidl_fuchsia_hardware_audio::PlugDetectCapabilities::CanAsyncNotify)
        );

        let mut formats_fut = proxy.get_dai_formats();
        // Should not be complete
        exec.run_until_stalled(&mut formats_fut)
            .expect_pending("shouldn't finish until codec polled");

        // Codec can answer this on it's own, this should not complete.
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");

        // Should now
        let formats =
            exec.run_singlethreaded(&mut formats_fut).expect("fidl succeed").expect("format ok");
        let Some(formats) = formats.first() else {
            panic!("Expected at least one format");
        };
        assert_eq!(formats.number_of_channels.len(), 2);
        assert_eq!(formats.frame_rates[0], 48000);

        let mut set_format_fut = proxy.set_dai_format(&fidl_fuchsia_hardware_audio::DaiFormat {
            number_of_channels: 2,
            channels_to_use_bitmask: 0x3,
            sample_format: DaiSampleFormat::PcmUnsigned,
            frame_format: DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
            frame_rate: 48000,
            bits_per_slot: 16,
            bits_per_sample: 16,
        });

        // should not complete
        exec.run_until_stalled(&mut set_format_fut).expect_pending("should pend");

        // Should make a request
        // Codec can answer this on it's own, this should not complete.
        let Some(Ok(CodecRequest::SetFormat { format, responder })) =
            exec.run_singlethreaded(&mut codec_next_fut)
        else {
            panic!("Expected a SetFormat request");
        };

        assert_eq!(format.number_of_channels, 2);
        assert_eq!(format.channels_to_use_bitmask, 0x3);

        exec.run_until_stalled(&mut set_format_fut).expect_pending("should pend");
        responder(Ok(()));

        let Ok(Ok(codec_format_info)) = exec.run_singlethreaded(&mut set_format_fut) else {
            panic!("Expected ok response");
        };

        assert_eq!(codec_format_info, fidl_fuchsia_hardware_audio::CodecFormatInfo::default());

        let mut codec_next_fut = codec.next();
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");

        let mut start_fut = proxy.start();
        exec.run_until_stalled(&mut start_fut).expect_pending("should pend");

        let Some(Ok(CodecRequest::Start { responder })) =
            exec.run_singlethreaded(&mut codec_next_fut)
        else {
            panic!("Expected a Start request");
        };
        let mut codec_next_fut = codec.next();

        exec.run_until_stalled(&mut start_fut).expect_pending("should pend");

        let started_time = fasync::MonotonicInstant::now();
        responder(Ok(started_time.into()));

        let time = exec.run_until_stalled(&mut start_fut).expect("should be started").unwrap();

        assert_eq!(started_time.into_nanos(), time);

        // Asking to start again should return the same start time.
        let mut start_fut = proxy.start();
        exec.run_until_stalled(&mut start_fut).expect_pending("should pend");
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");

        let time = exec.run_until_stalled(&mut start_fut).expect("should be started").unwrap();
        assert_eq!(started_time.into_nanos(), time);

        let mut watch_plug_state_fut = proxy.watch_plug_state();
        exec.run_until_stalled(&mut watch_plug_state_fut).expect_pending("should pend");
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");
        let plug_state =
            exec.run_until_stalled(&mut watch_plug_state_fut).expect("should finish").expect("ok");

        assert_eq!(plug_state.plugged, Some(true));
        assert!(
            plug_state.plug_state_time.unwrap() <= fasync::MonotonicInstant::now().into_nanos()
        );

        // Second watch plug state should hang.
        let mut watch_plug_state_fut = proxy.watch_plug_state();
        exec.run_until_stalled(&mut watch_plug_state_fut).expect_pending("should pend");
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");
        exec.run_until_stalled(&mut watch_plug_state_fut).expect_pending("should pend");

        drop(codec_next_fut);

        // Until we send a plug update.
        codec.update_plug_state(false).unwrap();

        let plug_state =
            exec.run_until_stalled(&mut watch_plug_state_fut).expect("done").expect("ok");

        assert_eq!(plug_state.plugged, Some(false));

        let mut codec_next_fut = codec.next();

        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");

        // Check on the health, but there's nothing interesting about the response.
        let mut health_fut = proxy.get_health_state();
        exec.run_until_stalled(&mut health_fut).expect_pending("should pend");
        exec.run_until_stalled(&mut codec_next_fut).expect_pending("no event");
        let _health = exec.run_until_stalled(&mut health_fut).expect("ready").expect("ok");

        let mut stop_fut = proxy.stop();
        let Poll::Ready(Some(Ok(CodecRequest::Stop { responder }))) =
            exec.run_until_stalled(&mut codec_next_fut)
        else {
            panic!("Expected a codec request to stop");
        };
        exec.run_until_stalled(&mut stop_fut).expect_pending("should pend");

        let response_time = fasync::MonotonicInstant::now();
        responder(Ok(response_time.into()));

        let Poll::Ready(Ok(received_time)) = exec.run_until_stalled(&mut stop_fut) else {
            panic!("Expected stop to finish");
        };

        assert_eq!(received_time, response_time.into_nanos());
    }

    #[fuchsia::test]
    async fn started_twice_before_response() {
        let (mut codec, client) = SoftCodec::create(
            Some(&TEST_UNIQUE_ID),
            TEST_MANUF,
            TEST_PRODUCT,
            CodecDirection::Output,
            DaiSupportedFormats {
                number_of_channels: vec![1, 2],
                sample_formats: vec![DaiSampleFormat::PcmUnsigned],
                // TODO(https://fxbug.dev/278283913): Configuration for which FrameFormatStandard the chip expects
                frame_formats: vec![DaiFrameFormat::FrameFormatStandard(
                    DaiFrameFormatStandard::I2S,
                )],
                frame_rates: vec![48000],
                bits_per_slot: vec![16],
                bits_per_sample: vec![16],
            },
            true,
        );
        let proxy = client.into_proxy();

        let set_format_fut = proxy.set_dai_format(&fidl_fuchsia_hardware_audio::DaiFormat {
            number_of_channels: 2,
            channels_to_use_bitmask: 0x3,
            sample_format: DaiSampleFormat::PcmUnsigned,
            frame_format: DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S),
            frame_rate: 48000,
            bits_per_slot: 16,
            bits_per_sample: 16,
        });

        // Should make a request
        // Codec can answer this on it's own, this should not complete.
        let Some(Ok(CodecRequest::SetFormat { format, responder })) = codec.next().await else {
            panic!("Expected a SetFormat request");
        };

        assert_eq!(format.number_of_channels, 2);
        assert_eq!(format.channels_to_use_bitmask, 0x3);

        responder(Ok(()));

        let Ok(Ok(codec_format_info)) = set_format_fut.await else {
            panic!("Expeted ok response");
        };

        assert_eq!(codec_format_info, fidl_fuchsia_hardware_audio::CodecFormatInfo::default());

        let start_fut = proxy.start();

        let Some(Ok(CodecRequest::Start { responder })) = codec.next().await else {
            panic!("Expected a Start request");
        };

        let start_before_response_fut = proxy.start();
        // The codec stream closes.
        let codec_next = codec.next().await;
        let Some(Err(_)) = codec_next else {
            panic!("Expected stream to close on bad behavior from proxy: {codec_next:?}");
        };
        let codec_next = codec.next().await;
        let None = codec_next else {
            panic!("Expected stream to terminate after: {codec_next:?}");
        };
        // The start that shut us down should fail
        let start_before_response = start_before_response_fut.await;
        let Err(_) = start_before_response else {
            panic!(
                "Expected error from the second start on the proxy before a response: {start_before_response:?}"
            );
        };

        // The previous start call also will fail.
        let start_response = start_fut.await;
        let Err(_) = start_response else {
            panic!(
                "Expected error from the first start on the proxy before a response: {start_response:?}"
            );
        };
        // The responder should be ok to call even though it has no effect.
        let response_time = fasync::MonotonicInstant::now();
        responder(Ok(response_time.into()));
    }
}
