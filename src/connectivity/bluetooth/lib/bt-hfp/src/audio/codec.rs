// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fuchsia_audio_device::codec;
use fuchsia_audio_device::codec::CodecRequest;
use fuchsia_bluetooth::types::{PeerId, peer_audio_stream_id};
use fuchsia_sync::Mutex;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use log::{info, warn};
use std::sync::Arc;
use {
    fidl_fuchsia_audio_device as audio_device, fidl_fuchsia_hardware_audio as audio,
    fuchsia_async as fasync,
};

use super::{Control, ControlEvent, Error, HF_INPUT_UUID};
use crate::codec_id::CodecId;
use crate::sco;

struct CodecControlInner {
    start_request:
        Option<Box<dyn FnOnce(std::result::Result<zx::MonotonicInstant, zx::Status>) + Send>>,
    stop_request:
        Option<Box<dyn FnOnce(std::result::Result<zx::MonotonicInstant, zx::Status>) + Send>>,
    /// The earliest time that we will relay a start request after a stop.  Used to delay until the
    /// SCO is disconnected when encountering a start immediately after a stop.
    earliest_start_time: fasync::MonotonicInstant,
}

impl Default for CodecControlInner {
    fn default() -> Self {
        Self {
            start_request: None,
            stop_request: None,
            earliest_start_time: fasync::MonotonicInstant::INFINITE_PAST,
        }
    }
}

impl CodecControlInner {
    fn clear(&mut self) {
        *self = Self::default();
    }

    fn set_start_delay(&mut self) {
        self.earliest_start_time = fasync::MonotonicInstant::after(DELAY_AFTER_STOP);
    }
}

const DELAY_AFTER_STOP: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(1);

// Control that is connected to a Codec device registered with an
/// AudioDeviceRegistry component.  The AudioDeviceRegistry can request that we
/// start and/or stop the audio in-band which will send the request on to the
/// HFP task to initiate Audio Connection Setup when no call is in progress.
pub struct CodecControl {
    provider: audio_device::ProviderProxy,
    codec_task: Option<fasync::Task<()>>,
    events_sender: futures::channel::mpsc::Sender<ControlEvent>,
    events_receiver: Mutex<Option<futures::channel::mpsc::Receiver<ControlEvent>>>,
    codec_id: Option<CodecId>,
    connection: Option<sco::Connection>,
    connected_peer: Option<PeerId>,
    inner: Arc<Mutex<CodecControlInner>>,
}

impl Control for CodecControl {
    fn start(
        &mut self,
        id: PeerId,
        connection: sco::Connection,
        codec: crate::codec_id::CodecId,
    ) -> Result<(), Error> {
        if self.connection.is_some() {
            return Err(Error::AlreadyStarted);
        }
        if Some(codec) != self.codec_id {
            return Err(Error::UnsupportedParameters {
                source: anyhow!("CodecId must match connected CodecId"),
            });
        }
        if Some(id) != self.connected_peer {
            return Err(Error::UnsupportedParameters {
                source: anyhow!("Can't start a non-connected peer"),
            });
        };
        let Some(start_request) = self.inner.lock().start_request.take() else {
            return Err(Error::UnsupportedParameters {
                source: anyhow!("Can only start in response to request"),
            });
        };
        self.connection = Some(connection);
        start_request(Ok(fuchsia_async::MonotonicInstant::now().into()));
        Ok(())
    }

    fn stop(&mut self, id: PeerId) -> Result<(), Error> {
        if self.connection.is_none() {
            return Err(Error::NotStarted);
        }
        if Some(id) != self.connected_peer {
            return Err(Error::UnsupportedParameters {
                source: anyhow!("Can't stop a non-connected peer"),
            });
        }
        let Some(stop_request) = self.inner.lock().stop_request.take() else {
            return Err(Error::UnsupportedParameters {
                source: anyhow!("Codec can only stop in response to request"),
            });
        };
        self.connection = None;
        self.inner.lock().set_start_delay();
        stop_request(Ok(fuchsia_async::MonotonicInstant::now().into()));
        Ok(())
    }

    fn connect(&mut self, id: PeerId, supported_codecs: &[CodecId]) {
        let supported_formats: audio::DaiSupportedFormats;
        if supported_codecs.contains(&CodecId::MSBC) {
            self.codec_id = Some(CodecId::MSBC);
            supported_formats = CodecId::MSBC.try_into().unwrap();
        } else {
            self.codec_id = Some(CodecId::CVSD);
            supported_formats = CodecId::CVSD.try_into().unwrap();
        };
        let audio_dev_id = peer_audio_stream_id(id, HF_INPUT_UUID);
        let (codec, client) = codec::SoftCodec::create(
            Some(&audio_dev_id),
            "Fuchsia",
            super::DEVICE_NAME,
            codec::CodecDirection::Duplex,
            supported_formats.clone(),
            true,
        );
        self.codec_task = Some(fasync::Task::local(codec_task(
            id,
            self.provider.clone(),
            codec,
            supported_formats,
            client,
            self.events_sender.clone(),
            self.inner.clone(),
        )));
        self.connected_peer = Some(id);
    }

    fn disconnect(&mut self, id: PeerId) {
        info!("Codec peer {id} disconnected, cleaning up");
        self.codec_task = None;
        self.connected_peer = None;
        self.connection = None;
        self.codec_id = None;
        self.inner.lock().clear();
    }

    fn take_events(&self) -> BoxStream<'static, ControlEvent> {
        self.events_receiver.lock().take().unwrap().boxed()
    }

    fn failed_request(&self, request: ControlEvent, _error: Error) {
        match request {
            ControlEvent::RequestStart { id: _ } => {
                let Some(start_request) = self.inner.lock().start_request.take() else {
                    return;
                };
                start_request(Err(zx::Status::INTERNAL));
            }
            ControlEvent::RequestStop { id: _ } => {
                let Some(stop_request) = self.inner.lock().start_request.take() else {
                    return;
                };
                stop_request(Err(zx::Status::INTERNAL));
            }
            _ => unreachable!(),
        }
    }
}

async fn codec_task(
    id: PeerId,
    provider: audio_device::ProviderProxy,
    mut codec: codec::SoftCodec,
    supported_formats: audio::DaiSupportedFormats,
    client: fidl::endpoints::ClientEnd<audio::CodecMarker>,
    mut event_sender: futures::channel::mpsc::Sender<ControlEvent>,
    inner: Arc<Mutex<CodecControlInner>>,
) {
    let result = provider
        .add_device(audio_device::ProviderAddDeviceRequest {
            device_name: Some(super::DEVICE_NAME.into()),
            device_type: Some(audio_device::DeviceType::Codec),
            driver_client: Some(audio_device::DriverClient::Codec(client)),
            ..Default::default()
        })
        .await;
    match result {
        Err(e) => {
            warn!("FIDL Error adding device: {e:?}");
            return;
        }
        Ok(Err(e)) => {
            warn!("Failed to add device: {e:?}");
            return;
        }
        Ok(Ok(_)) => {}
    };
    info!("Added Codec device!");
    while let Some(event) = codec.next().await {
        let Ok(event) = event else {
            let _ = event_sender
                .send(ControlEvent::Stopped {
                    id,
                    error: Some(Error::audio_core(event.err().unwrap().into())),
                })
                .await;
            return;
        };
        info!("Codec request: {event:?}");
        let audio_event = match event {
            CodecRequest::SetFormat { format, responder } => {
                if supported_formats.number_of_channels.contains(&format.number_of_channels)
                    && supported_formats.frame_formats.contains(&format.frame_format)
                    && supported_formats.sample_formats.contains(&format.sample_format)
                    && supported_formats.frame_rates.contains(&format.frame_rate)
                    && supported_formats.bits_per_slot.contains(&format.bits_per_slot)
                    && supported_formats.bits_per_sample.contains(&format.bits_per_sample)
                {
                    responder(Ok(()));
                } else {
                    responder(Err(zx::Status::NOT_SUPPORTED));
                }
                continue;
            }
            CodecRequest::Start { responder } => {
                if inner.lock().start_request.is_some() {
                    responder(Err(zx::Status::ALREADY_EXISTS));
                    continue;
                }
                inner.lock().start_request = Some(responder);
                let deadline = inner.lock().earliest_start_time.clone();
                let now = fasync::MonotonicInstant::now();
                if deadline > now {
                    info!(
                        "Delaying start request {}ms due to too-fast stop->start cycle",
                        (deadline - now).into_millis()
                    );
                }
                fasync::Timer::new(deadline).await;
                ControlEvent::RequestStart { id }
            }
            CodecRequest::Stop { responder } => {
                if inner.lock().stop_request.is_some() {
                    responder(Err(zx::Status::ALREADY_EXISTS));
                    continue;
                }
                inner.lock().stop_request = Some(responder);
                ControlEvent::RequestStop { id }
            }
        };
        let _ = event_sender.send(audio_event).await;
    }
    warn!("Codec device finished, dropping..!");
}

impl CodecControl {
    pub fn new(provider: audio_device::ProviderProxy) -> Self {
        let (events_sender, receiver) = futures::channel::mpsc::channel(1);
        Self {
            provider,
            codec_task: None,
            events_sender,
            events_receiver: Mutex::new(Some(receiver)),
            inner: Default::default(),
            codec_id: None,
            connection: None,
            connected_peer: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fidl::endpoints::Proxy;
    use fidl_fuchsia_audio_device::ProviderRequestStream;
    use fixture::fixture;
    use futures::FutureExt;
    use futures::task::{Context, Poll};

    use crate::sco::test_utils::connection_for_codec;

    async fn connect_peer_to_codec(
        codec: &mut CodecControl,
        provider_requests: &mut ProviderRequestStream,
        codecs: &[CodecId],
    ) -> audio::CodecProxy {
        codec.connect(PeerId(1), codecs);

        let Some(Ok(audio_device::ProviderRequest::AddDevice {
            payload:
                audio_device::ProviderAddDeviceRequest {
                    driver_client: Some(client),
                    device_name: Some(_name),
                    device_type: Some(device_type),
                    ..
                },
            responder,
        })) = provider_requests.next().await
        else {
            panic!("Expected a request from the connect");
        };

        assert_eq!(device_type, audio_device::DeviceType::Codec);

        responder.send(Ok(&Default::default())).expect("response to succeed");

        let audio_device::DriverClient::Codec(codec_client) = client else {
            panic!("Should have provided a codec client");
        };
        codec_client.into_proxy()
    }

    async fn codec_setup_connected<F, Fut>(_test_name: &str, test: F)
    where
        F: FnOnce(audio::CodecProxy, CodecControl, ProviderRequestStream) -> Fut,
        Fut: futures::Future<Output = ()>,
    {
        let (provider_proxy, mut provider_requests) =
            fidl::endpoints::create_proxy_and_stream::<audio_device::ProviderMarker>();
        let mut codec = CodecControl::new(provider_proxy);

        let codec_proxy =
            connect_peer_to_codec(&mut codec, &mut provider_requests, &[CodecId::MSBC]).await;

        test(codec_proxy, codec, provider_requests).await
    }

    #[fixture(codec_setup_connected)]
    #[fuchsia::test]
    async fn publishes_on_connect(
        codec_client: audio::CodecProxy,
        codec: CodecControl,
        _provider_requests: ProviderRequestStream,
    ) {
        let _properties = codec_client.get_properties().await.unwrap();
        let audio::CodecGetDaiFormatsResult::Ok(formats) =
            codec_client.get_dai_formats().await.unwrap()
        else {
            panic!("Expected formats from get_dai_formats");
        };

        assert_eq!(formats.len(), 1);
        // MSBC has a frame-rate of 16khz
        assert_eq!(formats[0].frame_rates[0], 16000);
        drop(codec);
    }

    #[fixture(codec_setup_connected)]
    #[fuchsia::test]
    async fn removed_on_disconnect(
        codec_client: audio::CodecProxy,
        mut codec: CodecControl,
        _provider_requests: ProviderRequestStream,
    ) {
        codec.disconnect(PeerId(1));
        let _ = codec_client.on_closed().await;
    }

    #[fixture(codec_setup_connected)]
    #[fuchsia::test]
    async fn start_request_lifetime_test(
        codec_client: audio::CodecProxy,
        codec: CodecControl,
        provider_requests: ProviderRequestStream,
    ) {
        start_request_lifetime(codec_client, codec, CodecId::MSBC, provider_requests).await
    }

    async fn start_request_lifetime(
        codec_client: audio::CodecProxy,
        mut codec: CodecControl,
        good_codec: CodecId,
        _provider_requests: ProviderRequestStream,
    ) {
        let mut event_stream = codec.take_events();
        // start without a request should fail
        let (connection, _stream) = connection_for_codec(PeerId(1), good_codec, false);
        let start_result = codec.start(PeerId(1), connection, good_codec);
        let Err(Error::UnsupportedParameters { .. }) = start_result else {
            panic!("Expected error from start before request");
        };

        // request comes in
        let mut start_fut = codec_client.start();
        let (waker, wake_count) = futures_test::task::new_count_waker();
        let Poll::Pending = start_fut.poll_unpin(&mut Context::from_waker(&waker)) else {
            panic!("Expected start to be pending");
        };

        let Some(ControlEvent::RequestStart { id }) = event_stream.next().await else {
            panic!("Expected start request from event stream");
        };
        assert_eq!(id, PeerId(1));

        let bad_codec: CodecId = 100u8.into();

        // starting with a wrong codec fails, and doesn't complete the future.
        let (connection, _stream) = connection_for_codec(PeerId(1), bad_codec, false);
        let start_result = codec.start(PeerId(1), connection, bad_codec);
        let Err(Error::UnsupportedParameters { .. }) = start_result else {
            panic!("Expected error from start before request");
        };
        assert_eq!(wake_count.get(), 0);

        // starting after works and then completes the request.
        let (connection, _stream) = connection_for_codec(PeerId(1), good_codec, false);
        codec.start(PeerId(1), connection, good_codec).expect("should start ok");

        let Poll::Ready(_) = start_fut.poll_unpin(&mut Context::from_waker(&waker)) else {
            panic!("Expected to get response back from start");
        };

        // Starting after started is no good either.
        let (connection, _stream) = connection_for_codec(PeerId(1), good_codec, false);
        let start_result = codec.start(PeerId(1), connection, good_codec);
        let Err(Error::AlreadyStarted) = start_result else {
            panic!("Expected error from start while started");
        };
    }

    #[fixture(codec_setup_connected)]
    #[fuchsia::test]
    async fn stop_request_lifetime(
        codec_client: audio::CodecProxy,
        mut codec: CodecControl,
        _provider_requests: ProviderRequestStream,
    ) {
        let mut event_stream = codec.take_events();
        // can't stop before we are started
        let Err(Error::NotStarted) = codec.stop(PeerId(1)) else {
            panic!("Expected to not be able tp start when stopped");
        };

        // request comes in
        let start_fut = codec_client.start();

        let Some(ControlEvent::RequestStart { .. }) = event_stream.next().await else {
            panic!("Expected start request from event stream");
        };

        // starting after works and then completes the request.
        let (connection, _stream) = connection_for_codec(PeerId(1), CodecId::MSBC, false);
        codec.start(PeerId(1), connection, CodecId::MSBC).expect("should start ok");
        let _ = start_fut.await.expect("start to succeed");

        // can't stop without a request
        let Err(Error::UnsupportedParameters { .. }) = codec.stop(PeerId(1)) else {
            panic!("expected to not be able to stop without a request");
        };

        // request to stop comes in
        let mut stop_fut = codec_client.stop();
        let (waker, _wake_count) = futures_test::task::new_count_waker();
        let Poll::Pending = stop_fut.poll_unpin(&mut Context::from_waker(&waker)) else {
            panic!("Expected stop to be pending");
        };

        let Some(ControlEvent::RequestStop { id }) = event_stream.next().await else {
            panic!("Expected stop request from event stream");
        };
        assert_eq!(id, PeerId(1));

        // Can't stop a peer that's not started.
        let _ = codec.stop(PeerId(2)).expect_err("shouldn't be able to stop a different peer");

        // can stop the one requested
        let stop_request_time = fasync::MonotonicInstant::now();
        codec.stop(PeerId(1)).expect("should be able to stop");

        let Poll::Ready(_) = stop_fut.poll_unpin(&mut Context::from_waker(&waker)) else {
            panic!("Expected to get response back from start");
        };
        // back to being able to not stop it again

        let Err(Error::NotStarted) = codec.stop(PeerId(1)) else {
            panic!("Expected to not be able tp start when stopped");
        };

        // A start coming in immediately doesn't get relayed right away.
        let _start_fut = codec_client.start();

        assert!(event_stream.next().now_or_never().is_none());

        let Some(ControlEvent::RequestStart { .. }) = event_stream.next().await else {
            panic!("Expected start request from event stream");
        };

        let fulfilled_time = fasync::MonotonicInstant::now();

        let actual_delay = fulfilled_time - stop_request_time;
        assert!(
            actual_delay > DELAY_AFTER_STOP,
            "expected delay before relaying request to start, only {}ms",
            actual_delay.into_millis()
        );
    }

    #[fixture(codec_setup_connected)]
    #[fuchsia::test]
    async fn disconnect_and_reconnect(
        mut codec_client: audio::CodecProxy,
        mut codec: CodecControl,
        mut provider_requests: ProviderRequestStream,
    ) {
        codec.disconnect(PeerId(1));

        let _ = codec_client.on_closed().await;

        // Reconnect, but with a different codec.  It should still succeed.
        codec_client =
            connect_peer_to_codec(&mut codec, &mut provider_requests, &[CodecId::CVSD]).await;
        // Should be able to do the whole start request lifetime again now
        start_request_lifetime(codec_client, codec, CodecId::CVSD, provider_requests).await
    }
}
