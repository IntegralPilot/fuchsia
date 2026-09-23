// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use bt_a2dp::codec::MediaCodecConfig;
use bt_a2dp::media_task::*;
use bt_avdtp::MediaStream;
use fidl_fuchsia_bluetooth_bredr::AudioOffloadExtProxy;
use fidl_fuchsia_media::{AudioChannelId, AudioPcmMode, PcmFormat};
use fuchsia_async as fasync;
use fuchsia_audio_device::AudioStreamItem;
use fuchsia_bluetooth::inspect::DataStreamInspect;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect::Node;
use fuchsia_inspect_derive::{AttachError, Inspect};
use fuchsia_sync::Mutex;
use fuchsia_trace as trace;
use futures::channel::oneshot;
use futures::future::{BoxFuture, Shared, WeakShared};
use futures::task::{Context, Poll};
use futures::{FutureExt, SinkExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use log::{info, trace, warn};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::encoding::EncodedStream;
use crate::media::AudioSourceType;

pub mod audio_out_stream;
pub mod big_ben_stream;

pub trait AudioSourceStream:
    Stream<Item = fuchsia_audio_device::Result<AudioStreamItem>> + Send + Unpin
{
    fn watch_active(&self) -> BoxFuture<'static, bool> {
        futures::future::ready(true).boxed()
    }
}

pub trait AudioSourceStreamBuilder: Send + Sync {
    fn build(
        &self,
        peer_id: &PeerId,
        pcm_format: PcmFormat,
        external_delay: std::time::Duration,
        inspect_parent: &mut Node,
    ) -> Result<Box<dyn AudioSourceStream>, MediaTaskError>;
}

/// Builder is a MediaTaskBuilder will build `ConfiguredTask`s when configured.
/// `source_type` determines where the source of audio is provided.
/// `aac_available` determines whether AAC is advertised as supported.
/// When configured, a test stream is created to confirm that it is possible to stream audio using
/// the configuration.  This stream is discarded and the stream is restarted when the resulting
/// `ConfiguredTask` is started.
/// TODO(https://fxbug.dev/42145257): Avoid this creation / destruction on configure
#[derive(Clone)]
pub struct Builder {
    /// The type of source audio, for inspecting.
    source_type: AudioSourceType,
    /// The builder for the audio source stream.
    stream_builder: Arc<Box<dyn AudioSourceStreamBuilder>>,
    /// Whether AAC has been detected to be available
    aac_available: bool,
}

fn build_sbc_source() -> MediaCodecConfig {
    use bt_a2dp::media_types::*;
    let sbc_codec_info = SbcCodecInfo::new(
        SbcSamplingFrequency::FREQ48000HZ,
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

fn build_aac_source(bitrate: u32) -> MediaCodecConfig {
    use bt_a2dp::media_types::*;
    let codec_info = AacCodecInfo::new(
        AacObjectType::MANDATORY_SRC,
        AacSamplingFrequency::FREQ48000HZ,
        AacChannels::TWO,
        true,
        bitrate,
    )
    .unwrap();
    (&bt_avdtp::ServiceCapability::MediaCodec {
        media_type: bt_avdtp::MediaType::Audio,
        codec_type: bt_avdtp::MediaCodecType::AUDIO_AAC,
        codec_extra: codec_info.to_bytes().to_vec(),
    })
        .try_into()
        .unwrap()
}

async fn test_encodable(config: &MediaCodecConfig) -> Result<(), MediaTaskError> {
    // all sinks must support these options for audio input
    let required_format = PcmFormat {
        pcm_mode: AudioPcmMode::Linear,
        bits_per_sample: 16,
        frames_per_second: 48000,
        channel_map: vec![AudioChannelId::Lf],
    };
    EncodedStream::test(required_format, config).await
}

impl MediaTaskBuilder for Builder {
    fn configure(
        &self,
        peer_id: &PeerId,
        codec_config: &MediaCodecConfig,
    ) -> Result<Box<dyn MediaTaskRunner>, MediaTaskError> {
        let res = self.configure_task(&peer_id, codec_config);
        Ok::<Box<dyn MediaTaskRunner>, _>(Box::new(res?))
    }

    fn direction(&self) -> bt_avdtp::EndpointType {
        bt_avdtp::EndpointType::Source
    }

    fn supported_configs(
        &self,
        _peer_id: &PeerId,
        _offload: Option<AudioOffloadExtProxy>,
    ) -> BoxFuture<'static, Result<Vec<MediaCodecConfig>, MediaTaskError>> {
        // SBC is required to be supported to use this Builder
        let media_configs = if self.aac_available {
            vec![build_aac_source(crate::MAX_BITRATE_AAC), build_sbc_source()]
        } else {
            vec![build_sbc_source()]
        };
        futures::future::ready(Ok(media_configs)).boxed()
    }
}

impl Builder {
    /// Make a new builder that will source audio from `source_type`.
    pub async fn new(
        source_type: AudioSourceType,
        mut aac_available: bool,
    ) -> Result<Self, MediaTaskError> {
        let stream_builder: Box<dyn AudioSourceStreamBuilder> = match source_type {
            AudioSourceType::AudioOut => Box::new(audio_out_stream::AudioOutStream {}),
            AudioSourceType::BigBen => Box::new(big_ben_stream::BigBenStream::default()),
            AudioSourceType::Offload => {
                return Err(MediaTaskError::Other("Offload source not supported".to_string()));
            }
        };

        // Check to see that we can encode SBC audio.
        // This is a requirement of A2DP 1.3: Section 4.2
        test_encodable(&MediaCodecConfig::min_sbc()).await?;
        if aac_available {
            if let Err(e) = test_encodable(&build_aac_source(0)).await {
                warn!("AAC enabled by configuration but is not encodable: {e:?}, disabling");
                aac_available = false;
            }
        }
        Ok(Self { source_type, stream_builder: Arc::new(stream_builder), aac_available })
    }

    pub(crate) fn configure_task(
        &self,
        peer_id: &PeerId,
        codec_config: &MediaCodecConfig,
    ) -> Result<ConfiguredTask, MediaTaskError> {
        let channel_map = match codec_config.channel_count() {
            Ok(1) => vec![AudioChannelId::Cf],
            Ok(2) => vec![AudioChannelId::Lf, AudioChannelId::Rf],
            Ok(_) | Err(_) => return Err(MediaTaskError::NotSupported),
        };
        let pcm_format = PcmFormat {
            pcm_mode: AudioPcmMode::Linear,
            bits_per_sample: 16,
            frames_per_second: codec_config.sampling_frequency()?,
            channel_map,
        };
        let task = ConfiguredTask::build(
            pcm_format.clone(),
            self.source_type,
            self.stream_builder.clone(),
            peer_id.clone(),
            codec_config,
        );
        let test_stream = self.stream_builder.build(
            peer_id,
            pcm_format.clone(),
            Duration::ZERO,
            &mut Default::default(),
        )?;
        if let Err(e) = EncodedStream::build(&pcm_format, Box::pin(test_stream), codec_config) {
            trace!("inband_source::Builder: can't build encoded stream: {e:?}");
            return Err(e.context("Building test stream").into());
        }
        Ok(task)
    }
}

#[derive(Clone)]
struct SharedSourceStream(Arc<Mutex<Box<dyn AudioSourceStream>>>);

impl From<Box<dyn AudioSourceStream>> for SharedSourceStream {
    fn from(stream: Box<dyn AudioSourceStream>) -> Self {
        Self(Arc::new(Mutex::new(stream)))
    }
}

impl Stream for SharedSourceStream {
    type Item = fuchsia_audio_device::Result<AudioStreamItem>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.lock().poll_next_unpin(cx)
    }
}

/// Provides audio from this to the MediaStream when started. Streams are created when first needed
/// and preserved across stream starts and stops.
pub(crate) struct ConfiguredTask {
    /// The peer for this task
    peer_id: PeerId,
    /// The builder for the source_stream.
    stream_builder: Arc<Box<dyn AudioSourceStreamBuilder>>,
    /// The type of source audio.
    source_type: AudioSourceType,
    /// Audio source stream if started, preserved to restart on AudioDisabled.
    /// Shared with any RunningTask
    source_stream: OnceLock<SharedSourceStream>,
    /// Format the source audio should be produced in.
    pub(crate) pcm_format: PcmFormat,
    /// Configuration providing the format of encoded audio requested by the peer.
    codec_config: MediaCodecConfig,
    /// Delay reported from the peer. Defaults to zero. Passed on to the Audio source.
    delay: Duration,
    /// Future if the task is running or has ran and and the shared future has not been dropped.
    /// Used to indicate errors for set_delay as we currently do not support updating delays dynamically.
    running: Option<WeakShared<BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>>>>,
    /// Inspect node
    inspect: fuchsia_inspect::Node,
}

impl ConfiguredTask {
    /// Build a new ConfiguredTask. Usually only called by Builder.
    pub(crate) fn build(
        pcm_format: PcmFormat,
        source_type: AudioSourceType,
        stream_builder: Arc<Box<dyn AudioSourceStreamBuilder>>,
        peer_id: PeerId,
        codec_config: &MediaCodecConfig,
    ) -> Self {
        Self {
            peer_id,
            stream_builder,
            pcm_format,
            source_type,
            source_stream: OnceLock::new(),
            codec_config: codec_config.clone(),
            delay: Duration::ZERO,
            running: None,
            inspect: Default::default(),
        }
    }

    fn get_or_build_stream(&mut self) -> Result<&SharedSourceStream, MediaTaskError> {
        if self.source_stream.get().is_none() {
            // If this fails, it's because it already got set, we can throw out the new stream.
            let _ = self.source_stream.set(
                self.stream_builder
                    .build(&self.peer_id, self.pcm_format.clone(), self.delay, &mut self.inspect)?
                    .into(),
            );
        }
        Ok(self.source_stream.get().expect("initialized if none"))
    }

    fn update_inspect(&self) {
        self.inspect.record_string("source_type", &format!("{}", self.source_type));
        self.inspect.record_string("codec_config", &format!("{:?}", self.codec_config));
    }
}

impl Inspect for &mut ConfiguredTask {
    fn iattach(
        self,
        parent: &fuchsia_inspect::Node,
        name: impl AsRef<str>,
    ) -> Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());
        self.update_inspect();
        Ok(())
    }
}

impl MediaTaskRunner for ConfiguredTask {
    fn start(
        &mut self,
        stream: MediaStream,
        _offload: Option<AudioOffloadExtProxy>,
    ) -> Result<Box<dyn MediaTask>, MediaTaskError> {
        let source_stream = self.get_or_build_stream()?.clone();
        let encoded_stream =
            EncodedStream::build(&self.pcm_format, Box::pin(source_stream), &self.codec_config)
                .context("Building EncodedStream")?;
        let mut data_stream_inspect = DataStreamInspect::default();
        let _ = data_stream_inspect.iattach(&self.inspect, "data_stream");
        let stream_task = RunningTask::build(
            self.codec_config.clone(),
            encoded_stream,
            stream,
            data_stream_inspect,
        );
        self.running = stream_task.result_fut.downgrade();
        Ok(Box::new(stream_task))
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

    fn watch_active(&mut self) -> BoxFuture<'static, bool> {
        let Some(stream) = self.source_stream.get() else {
            return futures::future::ready(true).boxed();
        };
        let stream = stream.clone();
        async move {
            let fut = stream.0.lock().watch_active();
            fut.await
        }
        .boxed()
    }

    /// the running media task to the tree (i.e. data transferred, jitter, etc)
    fn iattach(&mut self, parent: &Node, name: &str) -> Result<(), AttachError> {
        fuchsia_inspect_derive::Inspect::iattach(self, parent, name)
    }
}

struct RunningTask {
    stream_task: Option<fasync::Task<()>>,
    result_fut: Shared<BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>>>,
}

impl RunningTask {
    /// The main streaming task. Reads encoded audio from the encoded_stream and packages into RTP
    /// packets, sending the resulting RTP packets using `media_stream`.
    async fn stream_task(
        codec_config: MediaCodecConfig,
        mut encoded_stream: EncodedStream,
        mut media_stream: MediaStream,
        mut data_stream_inspect: DataStreamInspect,
    ) -> Result<MediaTaskStatus, MediaTaskError> {
        data_stream_inspect.start();
        let frames_per_encoded = codec_config.pcm_frames_per_encoded_frame() as u32;
        let max_tx_size = media_stream.max_tx_size()?;
        let mut packet_builder = codec_config.make_packet_builder(max_tx_size)?;
        while let Some(item) =
            encoded_stream.try_next().await.map_err(|e| MediaTaskError::Other(e.to_string()))?
        {
            let encoded = match item {
                AudioStreamItem::Data(encoded) => encoded,
                AudioStreamItem::AudioDisabled => {
                    return Ok(MediaTaskStatus::AudioDisabled);
                }
            };

            let packets = match packet_builder.add_frame(encoded, frames_per_encoded) {
                Err(e) => {
                    warn!("Can't add packet to RTP packet: {:?}", e);
                    continue;
                }
                Ok(packets) => packets,
            };

            for packet in packets {
                trace::duration_begin!("bt-a2dp", "Media:PacketSent");
                if let Err(e) = media_stream.send(packet.to_vec()).await {
                    info!("Failed sending packet to peer: {}", e);
                    trace::duration_end!("bt-a2dp", "Media:PacketSent");
                    return Err(e.into());
                }
                data_stream_inspect
                    .record_transferred(packet.len(), fasync::MonotonicInstant::now());
                trace::duration_end!("bt-a2dp", "Media:PacketSent");
            }
        }
        Ok(MediaTaskStatus::Stopped)
    }

    fn build(
        codec_config: MediaCodecConfig,
        encoded_stream: EncodedStream,
        media_stream: MediaStream,
        inspect: DataStreamInspect,
    ) -> Self {
        let (sender, receiver) = oneshot::channel();
        let stream_task_fut =
            Self::stream_task(codec_config, encoded_stream, media_stream, inspect);
        let wrapped_task = fasync::Task::spawn(async move {
            trace::instant!("bt-a2dp", "Media:Start", trace::Scope::Thread);
            let result = stream_task_fut.await;
            let _ = sender.send(result);
        });
        let result_fut = receiver
            .map_ok_or_else(|_err| Ok(MediaTaskStatus::Stopped), |result| result)
            .boxed()
            .shared();
        Self { stream_task: Some(wrapped_task), result_fut }
    }
}

impl MediaTask for RunningTask {
    fn finished(&mut self) -> BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>> {
        self.result_fut.clone().boxed()
    }

    fn stop(&mut self) -> Result<MediaTaskStatus, MediaTaskError> {
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

    #[cfg(not(feature = "test_encoding"))]
    #[fuchsia::test]
    async fn test_encoding_fails_in_non_encoding_test_environment() {
        let builder_result = Builder::new(AudioSourceType::BigBen, false).await;

        assert!(builder_result.is_err());
    }

    #[cfg(feature = "test_encoding")]
    mod encoding {
        use super::*;

        use bt_a2dp::media_types::*;
        use bt_avdtp::MediaCodecType;
        use bt_channel_test_support::{Transport, create_test_channels};
        use fuchsia_inspect as inspect;
        use fuchsia_sync::{Mutex, RwLock};
        use futures::StreamExt;
        use std::sync::Arc;
        use test_case::test_case;
        use test_util::assert_gt;

        #[fuchsia::test]
        async fn configures_source_from_codec_config() {
            let builder = Builder::new(AudioSourceType::BigBen, false).await.expect("can encode");

            // Minimum SBC requirements are mono, 48kHz
            let mono_config = MediaCodecConfig::min_sbc();
            let task = builder.configure_task(&PeerId(1), &mono_config).expect("should build okay");
            assert_eq!(48000, task.pcm_format.frames_per_second);
            assert_eq!(1, task.pcm_format.channel_map.len());

            // A standard SBC audio config which is stereo and 44.1kHz
            let sbc_codec_info = SbcCodecInfo::new(
                SbcSamplingFrequency::FREQ44100HZ,
                SbcChannelMode::JOINT_STEREO,
                SbcBlockCount::SIXTEEN,
                SbcSubBands::EIGHT,
                SbcAllocation::LOUDNESS,
                SbcCodecInfo::BITPOOL_MIN,
                SbcCodecInfo::BITPOOL_MAX,
            )
            .unwrap();
            let stereo_config = MediaCodecConfig::build(
                MediaCodecType::AUDIO_SBC,
                &sbc_codec_info.to_bytes().to_vec(),
            )
            .unwrap();

            let task =
                builder.configure_task(&PeerId(1), &stereo_config).expect("should build okay");
            assert_eq!(44100, task.pcm_format.frames_per_second);
            assert_eq!(2, task.pcm_format.channel_map.len());
        }

        #[test_case(Transport::Socket ; "socket")]
        #[test_case(Transport::Fidl ; "fidl")]
        #[fuchsia::test]
        async fn source_media_stream_stats(transport: Transport) {
            let builder = Builder::new(AudioSourceType::BigBen, false).await.expect("can encode");

            let inspector = inspect::component::inspector();
            let root = inspector.root();

            // Minimum SBC requirements are mono, 48kHz
            let mono_config = MediaCodecConfig::min_sbc();
            let mut task =
                builder.configure_task(&PeerId(1), &mono_config).expect("should build okay");
            MediaTaskRunner::iattach(&mut task, &root, "source_task").expect("should attach okay");

            let (local, mut remote) = create_test_channels(transport);
            let local = Arc::new(RwLock::new(local));
            let weak_local = Arc::downgrade(&local);
            let stream = MediaStream::new(Arc::new(Mutex::new(true)), weak_local);

            let _running_task = task.start(stream, None).expect("media should start");

            let _ = remote.next().await;

            // Yield to let the sender task complete its continuation and update inspect.
            fasync::Timer::new(std::time::Duration::from_millis(50)).await;

            let hierarchy = inspect::reader::read(inspector).await.expect("read the inspect");

            // We don't know exactly how many were sent at this point, but make sure we got at
            // least some recorded.
            let total_bytes = hierarchy
                .get_property_by_path(&vec!["source_task", "data_stream", "total_bytes"])
                .expect("missing property");
            assert_gt!(total_bytes.uint().expect("uint"), 0);

            let bytes_per_second_current = hierarchy
                .get_property_by_path(&vec![
                    "source_task",
                    "data_stream",
                    "bytes_per_second_current",
                ])
                .expect("missing property");
            assert_gt!(bytes_per_second_current.uint().expect("uint"), 0);
        }

        struct MockAudioSourceStream {
            item_rx: Arc<Mutex<futures::channel::mpsc::UnboundedReceiver<AudioStreamItem>>>,
            active_rx: Arc<Mutex<futures::channel::mpsc::UnboundedReceiver<bool>>>,
        }

        impl Stream for MockAudioSourceStream {
            type Item = fuchsia_audio_device::Result<AudioStreamItem>;
            fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                match self.item_rx.lock().poll_next_unpin(cx) {
                    Poll::Ready(Some(item)) => Poll::Ready(Some(Ok(item))),
                    Poll::Ready(None) => Poll::Ready(None),
                    Poll::Pending => Poll::Pending,
                }
            }
        }

        impl AudioSourceStream for MockAudioSourceStream {
            fn watch_active(&self) -> BoxFuture<'static, bool> {
                let rx = self.active_rx.clone();
                futures::future::poll_fn(move |cx| match rx.lock().poll_next_unpin(cx) {
                    Poll::Ready(Some(v)) => Poll::Ready(v),
                    Poll::Ready(None) => Poll::Ready(false),
                    Poll::Pending => Poll::Pending,
                })
                .boxed()
            }
        }

        struct MockAudioSourceStreamBuilder {
            item_rx: Arc<Mutex<futures::channel::mpsc::UnboundedReceiver<AudioStreamItem>>>,
            active_rx: Arc<Mutex<futures::channel::mpsc::UnboundedReceiver<bool>>>,
        }

        impl AudioSourceStreamBuilder for MockAudioSourceStreamBuilder {
            fn build(
                &self,
                _peer_id: &PeerId,
                _pcm_format: PcmFormat,
                _external_delay: std::time::Duration,
                _inspect_parent: &mut Node,
            ) -> Result<Box<dyn AudioSourceStream>, MediaTaskError> {
                Ok(Box::new(MockAudioSourceStream {
                    item_rx: self.item_rx.clone(),
                    active_rx: self.active_rx.clone(),
                }))
            }
        }

        #[test_case(Transport::Socket ; "socket")]
        #[test_case(Transport::Fidl ; "fidl")]
        #[fuchsia::test]
        async fn running_task_ends_on_audio_disabled(transport: Transport) {
            let (item_tx, item_rx) = futures::channel::mpsc::unbounded();
            let (active_tx, active_rx) = futures::channel::mpsc::unbounded();
            let builder = MockAudioSourceStreamBuilder {
                item_rx: Arc::new(Mutex::new(item_rx)),
                active_rx: Arc::new(Mutex::new(active_rx)),
            };

            let mono_config = MediaCodecConfig::min_sbc();
            let pcm_format = PcmFormat {
                pcm_mode: AudioPcmMode::Linear,
                bits_per_sample: 16,
                frames_per_second: 48000,
                channel_map: vec![AudioChannelId::Cf],
            };

            let mut task = ConfiguredTask::build(
                pcm_format,
                AudioSourceType::AudioOut,
                Arc::new(Box::new(builder)),
                PeerId(1),
                &mono_config,
            );

            let (local, _remote) = create_test_channels(transport);
            let local = Arc::new(RwLock::new(local));
            let weak_local = Arc::downgrade(&local);
            let stream = MediaStream::new(Arc::new(Mutex::new(true)), weak_local);

            let mut running_task = task.start(stream, None).expect("media should start");

            // Disable audio channels via stream event
            item_tx.unbounded_send(AudioStreamItem::AudioDisabled).expect("send active status");

            // Task should end with AudioDisabled
            let result = running_task.finished().await;
            assert_eq!(result.expect("finished ok"), MediaTaskStatus::AudioDisabled);

            // Calling watch_active on the task waits for reactivation
            let watch_active_fut = task.watch_active();
            active_tx.unbounded_send(true).expect("send active status");
            assert_eq!(watch_active_fut.await, true);
        }
    }
}
