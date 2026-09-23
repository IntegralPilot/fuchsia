// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use bt_a2dp::codec::MediaCodecConfig;
use bt_a2dp::media_task::*;
use bt_avdtp::MediaStream;
use fidl_fuchsia_audio_device::ProviderProxy;
use fidl_fuchsia_bluetooth_bredr::{
    self as bredr, AudioOffloadControllerMarker, AudioOffloadControllerProxy, AudioOffloadExtProxy,
};
use fuchsia_async as fasync;
use fuchsia_bluetooth::types::{PeerId, peer_audio_stream_id};
use fuchsia_inspect::Node;
use fuchsia_inspect_derive::{AttachError, Inspect};
use fuchsia_trace as trace;
use futures::channel::oneshot;
use futures::future::{BoxFuture, Shared, WeakShared};
use futures::{FutureExt, TryFutureExt};
use log::{info, warn};
use std::time::Duration;

use crate::media::AUDIO_SOURCE_UUID;

/// Builder is a MediaTaskBuilder will build `ConfiguredTask`s when configured.
#[derive(Clone)]
pub struct Builder {
    provider: ProviderProxy,
}

fn build_sbc_source() -> MediaCodecConfig {
    use bt_a2dp::media_types::*;
    let sbc_codec_info = SbcCodecInfo::new(
        SbcSamplingFrequency::FREQ44100HZ,
        SbcChannelMode::JOINT_STEREO,
        SbcBlockCount::MANDATORY_SRC,
        SbcSubBands::MANDATORY_SRC,
        SbcAllocation::MANDATORY_SRC,
        SbcCodecInfo::BITPOOL_MIN,
        51, // Recommended bitpool value for 48khz Joint Stereo High Quality according to A2DP 1.4 Table 4.7
    )
    .unwrap();

    let codec_cap = bt_avdtp::ServiceCapability::MediaCodec {
        media_type: bt_avdtp::MediaType::Audio,
        codec_type: bt_avdtp::MediaCodecType::AUDIO_SBC,
        codec_extra: sbc_codec_info.to_bytes().to_vec(),
    };

    (&codec_cap).try_into().unwrap()
}

impl MediaTaskBuilder for Builder {
    fn configure(
        &self,
        peer_id: &PeerId,
        codec_config: &MediaCodecConfig,
    ) -> Result<Box<dyn MediaTaskRunner>, MediaTaskError> {
        let res = self.configure_task(peer_id, codec_config);
        Ok::<Box<dyn MediaTaskRunner>, _>(Box::new(res?))
    }

    fn direction(&self) -> bt_avdtp::EndpointType {
        bt_avdtp::EndpointType::Source
    }

    fn supported_configs(
        &self,
        peer_id: &PeerId,
        offload: Option<AudioOffloadExtProxy>,
    ) -> BoxFuture<'static, Result<Vec<MediaCodecConfig>, MediaTaskError>> {
        let peer_id = peer_id.clone();
        let supported_fut = async move {
            let Some(offload) = offload else {
                warn!("Offload proxy is not present for supported_configs {peer_id}");
                return Err(MediaTaskError::NotSupported);
            };
            let bredr::AudioOffloadExtGetSupportedFeaturesResponse {
                audio_offload_features, ..
            } = offload
                .get_supported_features()
                .await
                .map_err(|e| MediaTaskError::Other(e.to_string()))?;
            let Some(features) = audio_offload_features else {
                warn!("Offload features missing for {peer_id}: {audio_offload_features:?}");
                return Err(MediaTaskError::NotSupported);
            };
            // TODO(b/278283913): Enable AAC Audio as well when it's supported
            Ok(features
                .into_iter()
                .filter_map(|codec| match codec {
                    bredr::AudioOffloadFeatures::Sbc(_) => Some(build_sbc_source()),
                    _ => None,
                })
                .collect())
        };
        supported_fut.boxed()
    }
}

impl Builder {
    /// Make a new builder that will source audio from `source_type`.  See `sources::build_stream`
    /// for documentation on the types of streams that are available.
    pub fn new(provider: ProviderProxy) -> Result<Self, MediaTaskError> {
        Ok(Self { provider })
    }

    pub(crate) fn configure_task(
        &self,
        peer_id: &PeerId,
        codec_config: &MediaCodecConfig,
    ) -> Result<ConfiguredTask, MediaTaskError> {
        let configuration = match codec_config.codec_type() {
            &bt_avdtp::MediaCodecType::AUDIO_SBC => {
                let sampling_frequency = match codec_config.sampling_frequency() {
                    Ok(44100) => bredr::AudioSamplingFrequency::Hz44100,
                    Ok(48000) => bredr::AudioSamplingFrequency::Hz48000,
                    _ => return Err(MediaTaskError::NotSupported),
                };
                let channel_mode = match codec_config.channel_count() {
                    Ok(1) => bredr::AudioChannelMode::Mono,
                    Ok(2) => bredr::AudioChannelMode::Stereo,
                    _ => return Err(MediaTaskError::NotSupported),
                };
                let fidl_fuchsia_media::EncoderSettings::Sbc(settings) = codec_config
                    .encoder_settings()
                    .map_err(|e| MediaTaskError::Other(e.to_string()))?
                else {
                    return Err(MediaTaskError::NotSupported);
                };
                bredr::AudioOffloadConfiguration {
                    codec: Some(
                        bredr::AudioOffloadFeatures::Sbc(bredr::AudioSbcSupport::default()),
                    ),
                    max_latency: Some(200),
                    scms_t_enable: Some(false),
                    sampling_frequency: Some(sampling_frequency),
                    bits_per_sample: Some(bredr::AudioBitsPerSample::Bps16),
                    channel_mode: Some(channel_mode),
                    encoded_bit_rate: Some(328),
                    encoder_settings: Some(bredr::AudioEncoderSettings::Sbc(settings)),
                    ..Default::default()
                }
            }
            _ => return Err(MediaTaskError::NotSupported),
        };
        Ok(ConfiguredTask::build(
            *peer_id,
            configuration,
            codec_config.clone(),
            self.provider.clone(),
        ))
    }
}

/// Provides audio from this to the MediaStream when started.  Streams are created and started when
/// this task is started, and destroyed when stopped.
pub(crate) struct ConfiguredTask {
    /// Id of the peer that will be receiving the stream.
    peer_id: PeerId,
    /// Configuration providing the format of encoded audio requested by the peer.
    configuration: bredr::AudioOffloadConfiguration,
    /// Delay reported from the peer. Defaults to zero. Passed on to the media subsystem
    delay: Duration,
    /// Future if the task is running or has ran and and the shared future has not been dropped.
    /// Used to indicate errors for set_delay as we currently do not support updating delays dynamically.
    running: Option<WeakShared<BoxFuture<'static, Result<(), MediaTaskError>>>>,
    /// Inspect node
    inspect: fuchsia_inspect::Node,
    /// Provider to add Codec
    provider: ProviderProxy,
    /// Codec Config
    codec_config: MediaCodecConfig,
}

impl ConfiguredTask {
    /// Build a new ConfiguredTask.  Usually only called by Builder.
    /// `ConfiguredTask::start` will only return errors if the settings here cannot produce a
    /// stream.  No checks are done when building.
    pub(crate) fn build(
        peer_id: PeerId,
        configuration: bredr::AudioOffloadConfiguration,
        codec_config: MediaCodecConfig,
        provider: ProviderProxy,
    ) -> Self {
        Self {
            peer_id,
            configuration: configuration,
            delay: Duration::ZERO,
            running: None,
            inspect: Default::default(),
            provider,
            codec_config,
        }
    }

    // TODO(https://fxbug.dev/445946972): Add inspect details here
    fn update_inspect(&self) {
        self.inspect.record_bool("running", self.running.is_some());
    }
}

impl Inspect for &mut ConfiguredTask {
    fn iattach(
        self,
        parent: &fuchsia_inspect::Node,
        name: impl AsRef<str>,
    ) -> Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());
        self.inspect.record_string("peer_id", self.peer_id.to_string());
        self.inspect.record_string("codec_config", format!("{:?}", self.codec_config));
        self.update_inspect();
        Ok(())
    }
}

impl MediaTaskRunner for ConfiguredTask {
    fn start(
        &mut self,
        _stream: MediaStream,
        offload: Option<AudioOffloadExtProxy>,
    ) -> Result<Box<dyn MediaTask>, MediaTaskError> {
        let Some(offload_proxy) = offload else {
            warn!("Offload proxy is not present for start {}", self.peer_id);
            return Err(MediaTaskError::NotSupported);
        };
        let (controller_proxy, controller_server) =
            fidl::endpoints::create_proxy::<AudioOffloadControllerMarker>();
        if let Err(e) = offload_proxy.start_audio_offload(&self.configuration, controller_server) {
            return Err(MediaTaskError::Other(format!("Couldn't start audio offload: {e:?}")));
        }
        Ok(Box::new(RunningTask::build(
            self.codec_config.clone(),
            self.provider.clone(),
            self.peer_id,
            controller_proxy,
        )))
    }

    fn set_delay(&mut self, delay: Duration) -> Result<(), MediaTaskError> {
        if let Some(fut) = self.running.as_ref().and_then(WeakShared::upgrade) {
            // If the Shared isn't done, we are still running and can't update the delay.
            if fut.now_or_never().is_none() {
                return Err(MediaTaskError::NotSupported);
            }
        }
        self.delay = delay;
        Ok(())
    }

    fn iattach(&mut self, parent: &Node, name: &str) -> Result<(), AttachError> {
        fuchsia_inspect_derive::Inspect::iattach(self, parent, name)
    }
}

struct RunningTask {
    stream_task: Option<fasync::Task<()>>,
    result_fut: Shared<BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>>>,
    /// AudioOffloadController, used to stop offload and get indication of when
    /// started.
    offload_controller: AudioOffloadControllerProxy,
}

impl RunningTask {
    /// The main stream task. On an offload, this listens for codec start / stop and
    /// translates them into a request to start / stop offloading.
    // TODO(https://fxbug.dev/42153281): Move codec creation to earlier, respond to Start
    // TODO(https://fxbug.dev/445944147): Respond appropriately to a Stop here
    async fn stream_task(
        codec_config: MediaCodecConfig,
        provider: ProviderProxy,
        peer_id: PeerId,
    ) -> Result<(), Error> {
        info!("Stream Task Starting..");
        use fidl_fuchsia_audio_device::*;
        use fidl_fuchsia_hardware_audio::*;
        let device_id = peer_audio_stream_id(peer_id, AUDIO_SOURCE_UUID);
        let (mut soft_codec, codec_client) = fuchsia_audio_device::codec::SoftCodec::create(
            Some(&device_id),
            "Fuchsia",
            "Bluetooth A2DP",
            fuchsia_audio_device::codec::CodecDirection::Output,
            DaiSupportedFormats {
                number_of_channels: vec![codec_config.channel_count().unwrap() as u32],
                sample_formats: vec![DaiSampleFormat::PcmSigned],
                frame_formats: vec![DaiFrameFormat::FrameFormatStandard(
                    DaiFrameFormatStandard::I2S,
                )],
                frame_rates: vec![codec_config.sampling_frequency().unwrap()],
                bits_per_slot: vec![16],
                bits_per_sample: vec![16],
            },
            true,
        );
        let codec_task = fasync::Task::local(async move {
            loop {
                use fuchsia_audio_device::codec::CodecRequest;
                use futures::StreamExt;
                let codec_request = soft_codec.next().await;
                let Some(request) = codec_request else {
                    warn!("Codec Ended");
                    return Ok(());
                };
                let Ok(request) = request else {
                    let e = request.err().unwrap();
                    warn!("Error from Codec: {e:?}");
                    return Err(e.into());
                };
                info!("Handling Codec Request: {request:?}");
                match request {
                    CodecRequest::SetFormat { format: _, responder } => {
                        responder(Ok(()));
                    }
                    CodecRequest::Start { responder } => {
                        responder(Ok(fasync::MonotonicInstant::now().into()));
                    }
                    CodecRequest::Stop { responder } => {
                        responder(Ok(fasync::MonotonicInstant::now().into()));
                    }
                }
            }
        });
        let result = provider
            .add_device(ProviderAddDeviceRequest {
                device_name: Some(String::from("Bluetooth A2DP")),
                device_type: Some(fidl_fuchsia_audio_device::DeviceType::Codec),
                driver_client: Some(DriverClient::Codec(codec_client)),
                ..Default::default()
            })
            .await;
        match result {
            Err(e) => return Err(anyhow::format_err!("FIDL Error: {e:?}")),
            Ok(Err(e)) => return Err(anyhow::format_err!("failed to add device: {e:?}")),
            _ => {}
        }
        codec_task.await
    }

    fn build(
        codec_config: MediaCodecConfig,
        provider: ProviderProxy,
        peer_id: PeerId,
        offload_controller: AudioOffloadControllerProxy,
    ) -> Self {
        let (sender, receiver) = oneshot::channel();
        let stream_task_fut = Self::stream_task(codec_config, provider, peer_id);
        let wrapped_task = fasync::Task::local(async move {
            trace::instant!("bt-a2dp", "Media:Start", trace::Scope::Thread);
            let result = stream_task_fut
                .await
                .map(|()| MediaTaskStatus::Stopped)
                .map_err(|e| MediaTaskError::Other(format!("Error in streaming audio: {}", e)));
            let _ = sender.send(result);
        });
        let result_fut = receiver
            .map_ok_or_else(|_err| Ok(MediaTaskStatus::Stopped), |result| result)
            .boxed()
            .shared();
        Self { stream_task: Some(wrapped_task), result_fut, offload_controller }
    }
}

impl MediaTask for RunningTask {
    fn finished(&mut self) -> BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>> {
        self.result_fut.clone().boxed()
    }

    fn stop(&mut self) -> Result<MediaTaskStatus, MediaTaskError> {
        // Send a stop, although dropping the offload controller (or this task) should also stop
        let _ = self.offload_controller.stop().now_or_never();
        if let Some(task) = self.stream_task.take() {
            trace::instant!("bt-a2dp", "Media:Stopped", trace::Scope::Thread);
            drop(task);
        }
        // Either a result already happened, or we will just have sent an Ok(MediaTaskStatus::Stopped) by dropping the result
        // sender
        self.result().unwrap_or(Ok(MediaTaskStatus::Stopped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl_fuchsia_audio_device::{
        DeviceType, DriverClient, ProviderAddDeviceRequest, ProviderAddDeviceResponse,
        ProviderRequest,
    };
    use fidl_fuchsia_bluetooth_bredr::{
        AudioOffloadExtGetSupportedFeaturesResponse, AudioOffloadExtRequest, AudioOffloadFeatures,
        AudioSbcSupport,
    };
    use fidl_fuchsia_hardware_audio::{CodecProperties, PlugDetectCapabilities};
    use fuchsia_sync::{Mutex, RwLock};
    use futures::StreamExt;
    use std::sync::Arc;
    use test_case::test_case;

    #[fuchsia::test]
    async fn fails_without_offload() {
        let (proxy, _stream) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_audio_device::ProviderMarker>();
        let builder = Builder::new(proxy).unwrap();
        assert!(builder.supported_configs(&PeerId(1), None).await.is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn supported_configs_fails_on_error() {
        let (proxy, _stream) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_audio_device::ProviderMarker>();
        let builder = Builder::new(proxy).unwrap();
        let (proxy, mut offload_stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_bluetooth_bredr::AudioOffloadExtMarker,
        >();

        let mut supported_configs_fut = builder.supported_configs(&PeerId(1), Some(proxy));

        assert!(
            fasync::TestExecutor::poll_until_stalled(&mut supported_configs_fut).await.is_pending()
        );

        match offload_stream.next().await {
            Some(Ok(AudioOffloadExtRequest::GetSupportedFeatures { responder: _ })) => {
                // Closing the stream here will cause a PEER_CLOSED error on the proxy.
                drop(offload_stream);
            }
            x => panic!("expected GetSupportedFeatures, got {x:?}"),
        };

        assert!(supported_configs_fut.await.is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn supported_features_fails_on_bad_features() {
        let (proxy, _stream) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_audio_device::ProviderMarker>();
        let builder = Builder::new(proxy).unwrap();
        let (proxy, mut offload_stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_bluetooth_bredr::AudioOffloadExtMarker,
        >();

        let mut supported_configs_fut = builder.supported_configs(&PeerId(1), Some(proxy.clone()));

        assert!(
            fasync::TestExecutor::poll_until_stalled(&mut supported_configs_fut).await.is_pending()
        );

        match offload_stream.next().await {
            Some(Ok(AudioOffloadExtRequest::GetSupportedFeatures { responder })) => {
                responder
                    .send(&AudioOffloadExtGetSupportedFeaturesResponse {
                        audio_offload_features: None,
                        ..Default::default()
                    })
                    .unwrap();
            }
            x => panic!("expected GetSupportedFeatures, got {x:?}"),
        };
        assert!(supported_configs_fut.await.is_err());

        // Empty list should be an empty vec not an error.
        let mut supported_configs_fut = builder.supported_configs(&PeerId(1), Some(proxy));
        assert!(
            fasync::TestExecutor::poll_until_stalled(&mut supported_configs_fut).await.is_pending()
        );
        match offload_stream.next().await {
            Some(Ok(AudioOffloadExtRequest::GetSupportedFeatures { responder })) => {
                responder
                    .send(&AudioOffloadExtGetSupportedFeaturesResponse {
                        audio_offload_features: Some(vec![]),
                        ..Default::default()
                    })
                    .unwrap();
            }
            x => panic!("expected GetSupportedFeatures, got {x:?}"),
        };
        assert!(supported_configs_fut.await.unwrap().is_empty());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test(allow_stalls = false)]
    async fn starts_offload_on_codec(transport: Transport) {
        let (proxy, mut provider_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_audio_device::ProviderMarker>();
        let builder = Builder::new(proxy).unwrap();
        let (proxy, mut offload_stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_bluetooth_bredr::AudioOffloadExtMarker,
        >();

        let mut supported_configs_fut = builder.supported_configs(&PeerId(1), Some(proxy.clone()));

        assert!(
            fasync::TestExecutor::poll_until_stalled(&mut supported_configs_fut).await.is_pending()
        );

        match offload_stream.next().await {
            Some(Ok(AudioOffloadExtRequest::GetSupportedFeatures { responder })) => {
                responder
                    .send(&AudioOffloadExtGetSupportedFeaturesResponse {
                        audio_offload_features: Some(vec![AudioOffloadFeatures::Sbc(
                            AudioSbcSupport::default(),
                        )]),
                        ..Default::default()
                    })
                    .unwrap();
            }
            x => panic!("expected GetSupportedFeatures, got {x:?}"),
        };

        let supported_configs = supported_configs_fut.await.unwrap();

        assert_eq!(1, supported_configs.len());

        let negotiated_config =
            MediaCodecConfig::negotiate(&supported_configs[0], &supported_configs[0]).unwrap();
        let mut configured_task = builder.configure_task(&PeerId(1), &negotiated_config).unwrap();

        assert!(configured_task.running.is_none());

        let (local, _remote) = create_test_channels(transport);
        let local = Arc::new(RwLock::new(local));
        let weak_local = Arc::downgrade(&local);
        let stream = MediaStream::new(Arc::new(Mutex::new(true)), weak_local);

        let _task = configured_task.start(stream, Some(proxy.clone())).unwrap();

        // Should get an offload proxy start
        let _offload_controller = match offload_stream.next().await {
            Some(Ok(AudioOffloadExtRequest::StartAudioOffload {
                configuration,
                controller,
                control_handle: _,
            })) => {
                assert_eq!(
                    Some(bredr::AudioOffloadFeatures::Sbc(bredr::AudioSbcSupport::default())),
                    configuration.codec
                );
                controller
            }
            x => panic!("expected to start audio offload, got {x:?}"),
        };

        // Expect to register the codec with the provider
        let driver_client = match provider_stream.next().await {
            Some(Ok(ProviderRequest::AddDevice {
                payload: ProviderAddDeviceRequest { device_name, device_type, driver_client, .. },
                responder,
            })) => {
                assert!(device_name.is_some());
                assert_eq!(Some(DeviceType::Codec), device_type);
                let client = match driver_client {
                    Some(DriverClient::Codec(codec)) => codec.into_proxy(),
                    x => panic!("Expected Some(DriverClient::Codec(..)) got {x:?}"),
                };
                responder.send(Ok(&ProviderAddDeviceResponse::default())).unwrap();
                client
            }
            x => panic!("Expected to get an added device, got {x:?}"),
        };

        match driver_client.get_properties().await {
            Ok(CodecProperties {
                is_input,
                manufacturer,
                product,
                unique_id,
                plug_detect_capabilities,
                ..
            }) => {
                assert_eq!(Some(false), is_input);
                assert_eq!(Some("Fuchsia".to_owned()), manufacturer);
                assert!(product.is_some());
                assert_eq!(Some(PlugDetectCapabilities::CanAsyncNotify), plug_detect_capabilities);
                assert!(unique_id.is_some());
            }
            x => panic!("Expected GetProperties to be Ok, got {x:?}"),
        };

        let formats = driver_client.get_dai_formats().await.unwrap().unwrap();
        assert_eq!(1, formats.len());

        let _starts = driver_client.start().await.unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn configure_task_fails_on_unsupported_sampling_frequency() {
        let (proxy, _stream) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_audio_device::ProviderMarker>();
        let builder = Builder::new(proxy).unwrap();
        let sbc_codec_info = bt_a2dp::media_types::SbcCodecInfo::new(
            bt_a2dp::media_types::SbcSamplingFrequency::FREQ16000HZ,
            bt_a2dp::media_types::SbcChannelMode::JOINT_STEREO,
            bt_a2dp::media_types::SbcBlockCount::SIXTEEN,
            bt_a2dp::media_types::SbcSubBands::EIGHT,
            bt_a2dp::media_types::SbcAllocation::LOUDNESS,
            bt_a2dp::media_types::SbcCodecInfo::BITPOOL_MIN,
            51,
        )
        .unwrap();
        let codec_cap = bt_avdtp::ServiceCapability::MediaCodec {
            media_type: bt_avdtp::MediaType::Audio,
            codec_type: bt_avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: sbc_codec_info.to_bytes().to_vec(),
        };
        let config: MediaCodecConfig = (&codec_cap).try_into().unwrap();
        assert!(builder.configure_task(&PeerId(1), &config).is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn configure_task_fails_for_non_sbc_codec() {
        let (proxy, _stream) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_audio_device::ProviderMarker>();
        let builder = Builder::new(proxy).unwrap();
        let aac_codec_info = bt_a2dp::media_types::AacCodecInfo::new(
            bt_a2dp::media_types::AacObjectType::MPEG2_AAC_LC,
            bt_a2dp::media_types::AacSamplingFrequency::FREQ44100HZ,
            bt_a2dp::media_types::AacChannels::TWO,
            false,
            0,
        )
        .unwrap();
        let codec_cap = bt_avdtp::ServiceCapability::MediaCodec {
            media_type: bt_avdtp::MediaType::Audio,
            codec_type: bt_avdtp::MediaCodecType::AUDIO_AAC,
            codec_extra: aac_codec_info.to_bytes().to_vec(),
        };
        let config: MediaCodecConfig = (&codec_cap).try_into().unwrap();
        assert!(builder.configure_task(&PeerId(1), &config).is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn running_task_error_from_provider() {
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_audio_device::ProviderMarker>();
        let (offload_proxy, _offload_stream) =
            fidl::endpoints::create_proxy_and_stream::<AudioOffloadControllerMarker>();
        let mut task = RunningTask::build(build_sbc_source(), proxy, PeerId(1), offload_proxy);

        let mut finished_fut = task.finished();
        assert!(fasync::TestExecutor::poll_until_stalled(&mut finished_fut).await.is_pending());

        // Dropping the stream should cause an error in the running task.
        drop(stream);

        assert!(finished_fut.await.is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn running_task_stop_terminates() {
        let (proxy, _stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_audio_device::ProviderMarker>();
        let (offload_proxy, _offload_stream) =
            fidl::endpoints::create_proxy_and_stream::<AudioOffloadControllerMarker>();
        let mut task = RunningTask::build(build_sbc_source(), proxy, PeerId(1), offload_proxy);

        assert!(task.stop().is_ok());
        assert!(task.finished().await.is_ok());
    }
}
