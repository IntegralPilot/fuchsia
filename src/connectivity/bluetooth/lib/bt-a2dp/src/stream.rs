// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use bt_avdtp::{
    self as avdtp, ErrorCode, ServiceCapability, ServiceCategory, StreamEndpoint, StreamEndpointId,
};
use fidl_fuchsia_bluetooth_bredr::AudioOffloadExtProxy;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect::{self as inspect, Property};
use fuchsia_inspect_derive::{AttachError, Inspect};
use futures::future::BoxFuture;
use futures::{FutureExt, TryFutureExt};
use log::{info, warn};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::codec::{CodecNegotiation, MediaCodecConfig};
use crate::media_task::{
    MediaTask, MediaTaskBuilder, MediaTaskError, MediaTaskRunner, MediaTaskStatus,
};

/// Manages a local StreamEndpoint and its associated media task, starting and stopping the
/// related media task in sync with the endpoint's configured or streaming state.
/// Note that this does not coordinate state with peer, which is done by bt_a2dp::Peer.
pub struct Stream {
    endpoint: StreamEndpoint,
    /// The builder for media tasks associated with this endpoint.
    media_task_builder: Arc<Box<dyn MediaTaskBuilder>>,
    /// The MediaTaskRunner for this endpoint, if it is configured.
    media_task_runner: Option<Box<dyn MediaTaskRunner>>,
    /// The MediaTask, if it is running.
    media_task: Option<Box<dyn MediaTask>>,
    /// The peer associated with this endpoint, if it is configured.
    /// Used during reconfiguration for MediaTask recreation.
    peer_id: Option<PeerId>,
    /// Inspect Node for this stream
    inspect: fuchsia_inspect::Node,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("endpoint", &self.endpoint)
            .field("peer_id", &self.peer_id)
            .field("has media_task", &self.media_task.is_some())
            .finish()
    }
}

impl Inspect for &mut Stream {
    // Set up the StreamEndpoint to update the state
    // The MediaTask node will be created when the media task is started.
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());

        let endpoint_state_prop = self.inspect.create_string("endpoint_state", "");
        let callback =
            move |stream: &StreamEndpoint| endpoint_state_prop.set(&format!("{:?}", stream));
        callback(self.endpoint());
        self.endpoint_mut().set_update_callback(Some(Box::new(callback)));
        Ok(())
    }
}

impl Stream {
    pub fn build(endpoint: StreamEndpoint, media_task_builder: Box<dyn MediaTaskBuilder>) -> Self {
        Self {
            endpoint,
            media_task_builder: Arc::new(media_task_builder),
            media_task_runner: None,
            media_task: None,
            peer_id: None,
            inspect: Default::default(),
        }
    }

    fn as_new(&self) -> Self {
        Self {
            endpoint: self.endpoint.as_new(),
            media_task_builder: self.media_task_builder.clone(),
            media_task_runner: None,
            media_task: None,
            peer_id: None,
            inspect: Default::default(),
        }
    }

    pub fn endpoint(&self) -> &StreamEndpoint {
        &self.endpoint
    }

    pub fn endpoint_mut(&mut self) -> &mut StreamEndpoint {
        &mut self.endpoint
    }

    fn media_codec_config(&self) -> Option<MediaCodecConfig> {
        find_codec_capability(self.endpoint.capabilities())
            .and_then(|x| MediaCodecConfig::try_from(x).ok())
    }

    /// Returns true if the config given is a supported configuration of this stream
    /// Used when the stream is being configured to a specific configuration
    fn config_supported(&self, config: &MediaCodecConfig) -> bool {
        let Some(supported) = self.media_codec_config() else {
            return false;
        };
        supported.supports(&config)
    }

    /// Returns true if this stream and the given config are compatible - a valid configuration
    /// of this stream can be found within the capabilities of the given config.
    fn config_compatible(&self, config: &MediaCodecConfig) -> bool {
        let Some(supported) = self.media_codec_config() else {
            return false;
        };
        MediaCodecConfig::negotiate(&supported, config).is_some()
    }

    fn build_media_task_runner(
        &self,
        peer_id: &PeerId,
        config: &MediaCodecConfig,
    ) -> Option<Box<dyn MediaTaskRunner>> {
        match self.media_task_builder.configure(peer_id, &config) {
            Err(e) => {
                warn!("Failed to build media task: {e:?}");
                None
            }
            Ok(mut media_task_runner) => {
                if let Err(e) = media_task_runner.iattach(&self.inspect, "media_task") {
                    info!("Media Task inspect: {e}");
                }
                Some(media_task_runner)
            }
        }
    }

    fn supported_config_from_capability(
        &self,
        requested_cap: &ServiceCapability,
    ) -> Option<MediaCodecConfig> {
        MediaCodecConfig::try_from(requested_cap).ok().filter(|c| self.config_supported(c))
    }

    pub fn configure(
        &mut self,
        peer_id: &PeerId,
        remote_id: &StreamEndpointId,
        capabilities: Vec<ServiceCapability>,
    ) -> Result<(), (ServiceCategory, ErrorCode)> {
        if self.media_task_runner.is_some() {
            return Err((ServiceCategory::None, ErrorCode::SepInUse));
        }
        let unsupported = ErrorCode::UnsupportedConfiguration;
        let codec_cap =
            find_codec_capability(&capabilities).ok_or((ServiceCategory::None, unsupported))?;
        let media_unsupported = (ServiceCategory::MediaCodec, unsupported);
        let config = self.supported_config_from_capability(codec_cap).ok_or(media_unsupported)?;
        self.media_task_runner =
            Some(self.build_media_task_runner(peer_id, &config).ok_or(media_unsupported)?);
        self.peer_id = Some(peer_id.clone());
        self.endpoint.configure(remote_id, capabilities).or_else(|e| {
            self.media_task_runner = None;
            Err(e)
        })
    }

    pub fn set_delay(&mut self, delay: Duration) -> Result<(), ErrorCode> {
        let Some(runner) = self.media_task_runner.as_mut() else {
            return Err(ErrorCode::BadState);
        };
        match runner.set_delay(delay) {
            Err(MediaTaskError::NotSupported) => Err(ErrorCode::NotSupportedCommand),
            Err(_) => Err(ErrorCode::BadState),
            Ok(()) => Ok(()),
        }
    }

    pub fn reconfigure(
        &mut self,
        capabilities: Vec<ServiceCapability>,
    ) -> Result<(), (ServiceCategory, ErrorCode)> {
        let bad_state = (ServiceCategory::None, ErrorCode::BadState);
        let _peer_id = self.peer_id.as_ref().ok_or(bad_state)?;
        if let Some(requested_codec_cap) = find_codec_capability(&capabilities) {
            let unsupported = (ServiceCategory::MediaCodec, ErrorCode::UnsupportedConfiguration);
            let requested =
                self.supported_config_from_capability(requested_codec_cap).ok_or(unsupported)?;
            self.media_task_runner
                .as_mut()
                .ok_or(bad_state)?
                .reconfigure(&requested)
                .or(Err(unsupported))?;
        }
        self.endpoint.reconfigure(capabilities)
    }

    fn media_runner_ref(&mut self) -> Result<&mut Box<dyn MediaTaskRunner>, ErrorCode> {
        self.media_task_runner.as_mut().ok_or(ErrorCode::BadState)
    }

    /// Attempt to start the endpoint.
    /// If the endpoint is successfully started, the media task is started and a future that
    /// will finish when the media task finishes is returned.
    pub fn start(
        &mut self,
    ) -> Result<BoxFuture<'static, Result<MediaTaskStatus, Error>>, ErrorCode> {
        let peer_id = self.peer_id.ok_or(ErrorCode::BadState)?;
        if self.media_task_runner.is_none() {
            return Err(ErrorCode::BadState);
        }
        let _ = self.endpoint.start()?;
        let transport = self.endpoint.take_transport().ok_or(ErrorCode::BadState)?;
        let offload = self.endpoint.audio_offload();
        let mut task = match self.media_runner_ref()?.start(transport, offload) {
            Ok(media_task) => media_task,
            Err(e) => {
                warn!("Failed to start media task: {e:?} {peer_id}");
                let _ = self.endpoint.suspend()?;
                return Err(ErrorCode::BadState);
            }
        };
        let finished = task.finished();
        self.media_task = Some(task);
        Ok(finished.map_err(Into::into).boxed())
    }

    /// Suspends the media processor and endpoint.
    pub fn suspend(&mut self) -> Result<(), ErrorCode> {
        self.endpoint.suspend()?;
        let _ = self.media_task.take().ok_or(ErrorCode::BadState)?.stop();
        Ok(())
    }

    /// Watch for active channel state changes on the media task runner.
    /// Resolves to true when active, false when inactive.
    pub fn watch_active(&mut self) -> BoxFuture<'static, bool> {
        let Some(runner) = self.media_task_runner.as_mut() else {
            return futures::future::ready(true).boxed();
        };
        runner.watch_active()
    }

    fn stop_media_task(&mut self) {
        if let Some(mut task) = self.media_task.take() {
            // Ignoring stop errors, best effort.
            let _ = task.stop();
        }
        self.media_task_runner = None;
        self.peer_id = None;
    }

    /// Releases the endpoint and stops the processing of audio.
    pub fn release(
        &mut self,
        responder: avdtp::SimpleResponder,
        peer: &avdtp::Peer,
    ) -> avdtp::Result<()> {
        self.stop_media_task();
        self.endpoint.release(responder, peer)
    }

    pub fn abort(&mut self) {
        self.stop_media_task();
        self.endpoint.abort()
    }

    pub async fn initiate_abort(&mut self, peer: &avdtp::Peer) {
        self.stop_media_task();
        self.endpoint.initiate_abort(peer).await
    }
}

fn find_codec_capability(capabilities: &[ServiceCapability]) -> Option<&ServiceCapability> {
    capabilities.iter().find(|cap| cap.category() == ServiceCategory::MediaCodec)
}

/// Iterator which generates SEIDs.  Used by StreamsBuilder to get valid SEIDs.
#[derive(Clone, Debug)]
struct SeidRangeFrom {
    from: u8,
}

impl Default for SeidRangeFrom {
    fn default() -> Self {
        Self { from: 1 }
    }
}

impl Iterator for SeidRangeFrom {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.from;
        if self.from == 0x3E {
            self.from = 0x01;
        } else {
            self.from += 1;
        }
        Some(res)
    }
}

/// Builds a set of streams, based on the capabilities of a set of MediaTaskBuilders that are
/// supported and configured by the system.
pub struct StreamsBuilder {
    builders: Vec<Box<dyn MediaTaskBuilder>>,
    seid_range: SeidRangeFrom,
    node: inspect::Node,
}

impl Default for StreamsBuilder {
    fn default() -> Self {
        Self {
            builders: Default::default(),
            seid_range: SeidRangeFrom { from: Self::START_SEID },
            node: Default::default(),
        }
    }
}

impl Clone for StreamsBuilder {
    fn clone(&self) -> Self {
        Self {
            builders: self.builders.clone(),
            node: Default::default(),
            seid_range: self.seid_range.clone(),
        }
    }
}

impl StreamsBuilder {
    // Randomly chosen by fair dice roll
    // TODO(https://fxbug.dev/337321738): Do better for randomizing this maybe
    const START_SEID: u8 = 8;

    /// Add a builder to the set of builders used to generate streams.
    pub fn add_builder(&mut self, builder: impl MediaTaskBuilder + 'static) {
        self.builders.push(Box::new(builder));
        self.node.record_uint("builders", self.builders.len() as u64);
    }

    pub async fn peer_streams(
        &self,
        peer_id: &PeerId,
        offload: Option<AudioOffloadExtProxy>,
    ) -> Result<Streams, MediaTaskError> {
        let mut streams = Streams::default();
        let mut seid_range = self.seid_range.clone();
        for builder in &self.builders {
            let endpoint_type = builder.direction();
            let supported_res = builder.supported_configs(peer_id, offload.clone()).await;
            let Ok(supported) = supported_res else {
                info!(e:? = supported_res.err().unwrap(); "Failed to get supported configs from builder, skipping");
                continue;
            };
            let codec_caps = supported.iter().map(ServiceCapability::from);
            for codec_cap in codec_caps {
                let capabilities = match endpoint_type {
                    avdtp::EndpointType::Source => vec![
                        ServiceCapability::MediaTransport,
                        ServiceCapability::DelayReporting,
                        codec_cap,
                    ],
                    avdtp::EndpointType::Sink => {
                        vec![ServiceCapability::MediaTransport, codec_cap]
                    }
                };
                let endpoint = avdtp::StreamEndpoint::new(
                    seid_range.next().unwrap(),
                    avdtp::MediaType::Audio,
                    endpoint_type,
                    capabilities,
                )?;
                streams.insert(Stream::build(endpoint, builder.clone()));
            }
        }
        Ok(streams)
    }

    pub async fn negotiation(
        &self,
        peer_id: &PeerId,
        offload: Option<AudioOffloadExtProxy>,
        preferred_direction: avdtp::EndpointType,
    ) -> Result<CodecNegotiation, Error> {
        let mut caps_available = Vec::new();
        for builder in &self.builders {
            caps_available.extend(
                builder
                    .supported_configs(peer_id, offload.clone())
                    .await?
                    .iter()
                    .map(ServiceCapability::from),
            );
        }
        Ok(CodecNegotiation::build(caps_available, preferred_direction)?)
    }
}

impl Inspect for &mut StreamsBuilder {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.node = parent.create_child(name.as_ref());
        self.node.record_uint("builders", self.builders.len() as u64);
        Ok(())
    }
}

/// A set of streams, indexed by their local endpoint ID.
#[derive(Default)]
pub struct Streams {
    streams: HashMap<StreamEndpointId, Stream>,
    inspect_node: fuchsia_inspect::Node,
}

impl fmt::Debug for Streams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Streams").field("streams", &self.streams).finish()
    }
}

impl Streams {
    /// Makes a copy of this set of streams, but with all streams copied with their states set to
    /// idle.
    pub fn as_new(&self) -> Self {
        let streams =
            self.streams.iter().map(|(id, stream)| (id.clone(), stream.as_new())).collect();
        Self { streams, ..Default::default() }
    }

    /// Returns true if there are no streams in the set.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Inserts a stream, indexing it by the local endpoint id.
    /// It replaces any other stream with the same endpoint id.
    pub fn insert(&mut self, stream: Stream) {
        let local_id = stream.endpoint().local_id().clone();
        if self.streams.insert(local_id.clone(), stream).is_some() {
            warn!("Replacing stream with local id {local_id}");
        }
    }

    /// Retrieves a reference to the Stream referenced by `id`, if the stream exists,
    pub fn get(&self, id: &StreamEndpointId) -> Option<&Stream> {
        self.streams.get(id)
    }

    /// Retrieves a mutable reference to the Stream referenced by `id`, if the stream exists,
    pub fn get_mut(&mut self, id: &StreamEndpointId) -> Option<&mut Stream> {
        self.streams.get_mut(id)
    }

    /// Returns a vector of information on all the contained streams.
    pub fn information(&self) -> Vec<avdtp::StreamInformation> {
        self.streams.values().map(|x| x.endpoint().information()).collect()
    }

    /// Returns streams that are in the open (established but not streaming) state
    pub fn open(&self) -> impl Iterator<Item = &Stream> {
        self.streams.values().filter(|s| s.endpoint().state() == avdtp::StreamState::Open)
    }

    /// Returns streams that are streaming.
    pub fn streaming(&self) -> impl Iterator<Item = &Stream> {
        self.streams.values().filter(|s| s.endpoint().state() == avdtp::StreamState::Streaming)
    }

    /// Finds streams in the set which are compatible with `codec_config`.
    pub fn compatible(&self, codec_config: MediaCodecConfig) -> impl Iterator<Item = &Stream> {
        self.streams.values().filter(move |s| s.config_compatible(&codec_config))
    }
}

impl Inspect for &mut Streams {
    // Attach self to `parent`
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect_node = parent.create_child(name.as_ref());
        for stream in self.streams.values_mut() {
            stream.iattach(&self.inspect_node, inspect::unique_name("stream_"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bt_channel_test_support::{Transport, create_test_channels};
    use fuchsia_async as fasync;
    use std::pin::pin;
    use std::task::Poll;
    use test_case::test_case;

    use crate::media_task::tests::TestMediaTaskBuilder;
    use crate::media_types::*;

    pub(crate) fn sbc_mediacodec_capability() -> avdtp::ServiceCapability {
        let sbc_codec_info = SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ48000HZ,
            SbcChannelMode::MONO | SbcChannelMode::JOINT_STEREO,
            SbcBlockCount::MANDATORY_SRC,
            SbcSubBands::MANDATORY_SRC,
            SbcAllocation::MANDATORY_SRC,
            SbcCodecInfo::BITPOOL_MIN,
            SbcCodecInfo::BITPOOL_MAX,
        )
        .expect("SBC codec info");

        ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: sbc_codec_info.to_bytes().to_vec(),
        }
    }

    pub(crate) fn aac_mediacodec_capability(bitrate: u32) -> avdtp::ServiceCapability {
        let codec_info = AacCodecInfo::new(
            AacObjectType::MANDATORY_SRC,
            AacSamplingFrequency::FREQ48000HZ,
            AacChannels::TWO,
            true,
            bitrate,
        )
        .expect("should work");
        ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_AAC,
            codec_extra: codec_info.to_bytes().to_vec(),
        }
    }

    pub(crate) fn make_sbc_endpoint(seid: u8, direction: avdtp::EndpointType) -> StreamEndpoint {
        StreamEndpoint::new(
            seid,
            avdtp::MediaType::Audio,
            direction,
            vec![avdtp::ServiceCapability::MediaTransport, sbc_mediacodec_capability()],
        )
        .expect("endpoint creation should succeed")
    }

    const LOW_BITRATE: u32 = 320_000;
    const HIGH_BITRATE: u32 = 393_216;

    pub(crate) fn make_aac_endpoint(seid: u8, direction: avdtp::EndpointType) -> StreamEndpoint {
        StreamEndpoint::new(
            seid,
            avdtp::MediaType::Audio,
            direction,
            vec![avdtp::ServiceCapability::MediaTransport, aac_mediacodec_capability(LOW_BITRATE)],
        )
        .expect("endpoint creation should succeed")
    }

    fn make_stream(seid: u8, codec_type: avdtp::MediaCodecType) -> Stream {
        let endpoint = match codec_type {
            avdtp::MediaCodecType::AUDIO_SBC => {
                make_sbc_endpoint(seid, avdtp::EndpointType::Source)
            }
            avdtp::MediaCodecType::AUDIO_AAC => {
                make_aac_endpoint(seid, avdtp::EndpointType::Source)
            }
            _ => panic!("Unsupported codec_type"),
        };
        Stream::build(endpoint, TestMediaTaskBuilder::new().builder())
    }

    #[fuchsia::test]
    fn streams_basic_functionality() {
        let mut streams = Streams::default();

        streams.insert(make_stream(1, avdtp::MediaCodecType::AUDIO_SBC));
        streams.insert(make_stream(6, avdtp::MediaCodecType::AUDIO_AAC));

        let first_id = 1_u8.try_into().expect("good id");
        let missing_id = 5_u8.try_into().expect("good id");

        assert!(streams.get(&first_id).is_some());
        assert!(streams.get(&missing_id).is_none());

        assert!(streams.get_mut(&first_id).is_some());
        assert!(streams.get_mut(&missing_id).is_none());

        let expected_info = vec![
            make_sbc_endpoint(1, avdtp::EndpointType::Source).information(),
            make_aac_endpoint(6, avdtp::EndpointType::Source).information(),
        ];

        let infos = streams.information();

        assert_eq!(expected_info.len(), infos.len());

        if infos[0].id() == &first_id {
            assert_eq!(expected_info[0], infos[0]);
            assert_eq!(expected_info[1], infos[1]);
        } else {
            assert_eq!(expected_info[0], infos[1]);
            assert_eq!(expected_info[1], infos[0]);
        }
    }

    #[fuchsia::test]
    fn streams_filters_compatible_codecs() {
        let mut streams = Streams::default();
        streams.insert(make_stream(1, avdtp::MediaCodecType::AUDIO_SBC));
        streams.insert(make_stream(6, avdtp::MediaCodecType::AUDIO_AAC));

        // Even if the other bitrate is higher, we can negotiate to the lower bitrate.
        let config_high_bitrate_aac =
            MediaCodecConfig::try_from(&aac_mediacodec_capability(HIGH_BITRATE)).unwrap();

        let compatible: Vec<_> = streams.compatible(config_high_bitrate_aac).collect();
        assert_eq!(compatible.len(), 1);
        let codec_capability = compatible[0]
            .endpoint()
            .capabilities()
            .iter()
            .find(|x| x.category() == avdtp::ServiceCategory::MediaCodec)
            .expect("should have a codec");
        assert_eq!(
            MediaCodecConfig::try_from(codec_capability).unwrap().codec_type(),
            &avdtp::MediaCodecType::AUDIO_AAC
        );
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn rejects_unsupported_configurations(transport: Transport) {
        // Needed to make fasync::Tasks.
        let _exec = fasync::TestExecutor::new();
        let mut builder = TestMediaTaskBuilder::new_reconfigurable();
        let mut stream =
            Stream::build(make_sbc_endpoint(1, avdtp::EndpointType::Source), builder.builder());

        // the default test stream only supports 48000hz
        let unsupported_sbc_codec_info = SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ44100HZ,
            SbcChannelMode::JOINT_STEREO,
            SbcBlockCount::SIXTEEN,
            SbcSubBands::EIGHT,
            SbcAllocation::LOUDNESS,
            53,
            53,
        )
        .expect("SBC codec info");

        let unsupported_caps = vec![ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: unsupported_sbc_codec_info.to_bytes().to_vec(),
        }];

        let peer_id = PeerId(1);
        let stream_id = 1.try_into().expect("StreamEndpointId");
        let res = stream.configure(&peer_id, &stream_id, unsupported_caps.clone());
        assert!(res.is_err());
        assert_eq!(
            res.err(),
            Some((ServiceCategory::MediaCodec, ErrorCode::UnsupportedConfiguration))
        );

        assert_eq!(
            stream.reconfigure(unsupported_caps.clone()),
            Err((ServiceCategory::None, ErrorCode::BadState))
        );

        let supported_sbc_codec_info = SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ48000HZ,
            SbcChannelMode::JOINT_STEREO,
            SbcBlockCount::SIXTEEN,
            SbcSubBands::EIGHT,
            SbcAllocation::LOUDNESS,
            53,
            53,
        )
        .expect("SBC codec info");

        let sbc_codec_cap = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: supported_sbc_codec_info.to_bytes().to_vec(),
        };

        let supported_caps = vec![ServiceCapability::MediaTransport, sbc_codec_cap.clone()];

        let res = stream.configure(&peer_id, &stream_id, supported_caps.clone());
        assert!(res.is_ok());

        // need to be in the open state for reconfigure
        assert!(stream.endpoint_mut().establish().is_ok());
        let (transport, _remote) = create_test_channels(transport);
        match stream.endpoint_mut().receive_channel(transport) {
            Ok(false) => {}
            Ok(true) => panic!("Only should be expecting one channel"),
            Err(e) => panic!("Expected channel to be accepted, got {:?}", e),
        };

        assert_eq!(
            stream.reconfigure(unsupported_caps.clone()),
            Err((ServiceCategory::MediaCodec, ErrorCode::UnsupportedConfiguration))
        );

        let new_codec_caps = vec![ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: supported_sbc_codec_info.to_bytes().to_vec(),
        }];

        assert!(stream.reconfigure(new_codec_caps.clone()).is_ok());

        // Should be able to start after reconfigure, and we used the right configuration.
        let _ = stream.start().expect("stream should start ok");
        let task = builder.expect_task();
        assert_eq!(task.codec_config, MediaCodecConfig::try_from(&new_codec_caps[0]).unwrap());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn reconfigure_runner_fails(transport: Transport) {
        // Needed to make fasync::Tasks.
        let _exec = fasync::TestExecutor::new();
        let mut builder = TestMediaTaskBuilder::new();
        let mut stream =
            Stream::build(make_sbc_endpoint(1, avdtp::EndpointType::Source), builder.builder());

        let supported_sbc_codec_info = SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ48000HZ,
            SbcChannelMode::JOINT_STEREO,
            SbcBlockCount::SIXTEEN,
            SbcSubBands::EIGHT,
            SbcAllocation::LOUDNESS,
            53,
            53,
        )
        .expect("SBC codec info");

        let orig_codec_cap = ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: supported_sbc_codec_info.to_bytes().to_vec(),
        };

        let supported_caps = vec![ServiceCapability::MediaTransport, orig_codec_cap.clone()];

        let res = stream.configure(&PeerId(1), &(1.try_into().unwrap()), supported_caps.clone());
        assert!(res.is_ok());

        // need to be in the open state for reconfigure
        assert!(stream.endpoint_mut().establish().is_ok());
        let (transport, _remote) = create_test_channels(transport);
        match stream.endpoint_mut().receive_channel(transport) {
            Ok(false) => {}
            Ok(true) => panic!("Only should be expecting one channel"),
            Err(e) => panic!("Expected channel to be accepted, got {:?}", e),
        };

        // Should be able to start after configure, and we used the right configuration.
        let _ = stream.start().expect("stream should start ok");
        let task = builder.expect_task();
        assert_eq!(task.codec_config, MediaCodecConfig::try_from(&orig_codec_cap).unwrap());
        stream.suspend().expect("stream should suspend ok");

        // Try to reconfigure with a supported configuration, but the builder doesn't reconfigure.
        let mono_sbc_codec_info = SbcCodecInfo::new(
            SbcSamplingFrequency::FREQ48000HZ,
            SbcChannelMode::MONO,
            SbcBlockCount::SIXTEEN,
            SbcSubBands::EIGHT,
            SbcAllocation::LOUDNESS,
            53,
            53,
        )
        .expect("SBC codec info");

        let new_codec_caps = vec![ServiceCapability::MediaCodec {
            media_type: avdtp::MediaType::Audio,
            codec_type: avdtp::MediaCodecType::AUDIO_SBC,
            codec_extra: mono_sbc_codec_info.to_bytes().to_vec(),
        }];

        // Media Builder fails to reconfigure (as it's failing all reconfigures)
        assert_eq!(
            stream.reconfigure(new_codec_caps.clone()),
            Err((ServiceCategory::MediaCodec, ErrorCode::UnsupportedConfiguration))
        );

        // Should be able to start after reconfigure, but it will use the old configuration.
        let _ = stream.start().expect("stream should start ok");
        let task = builder.expect_task();
        assert_eq!(task.codec_config, MediaCodecConfig::try_from(&orig_codec_cap).unwrap());
        stream.suspend().expect("stream should suspend ok")
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn suspend_stops_media_task(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut task_builder = TestMediaTaskBuilder::new();
        let mut stream = Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            task_builder.builder(),
        );
        let next_task_fut = task_builder.next_task();
        let remote_id = 1_u8.try_into().expect("good id");

        let sbc_codec_cap = sbc_mediacodec_capability();
        let expected_codec_config =
            MediaCodecConfig::try_from(&sbc_codec_cap).expect("codec config");

        assert!(stream.configure(&PeerId(1), &remote_id, vec![]).is_err());
        assert!(stream.configure(&PeerId(1), &remote_id, vec![sbc_codec_cap]).is_ok());

        stream.endpoint_mut().establish().expect("establishment should start okay");
        let (transport, _remote) = create_test_channels(transport);
        let _ = stream.endpoint_mut().receive_channel(transport).expect("ready for a channel");

        assert!(stream.start().is_ok());

        // Task should be created here.
        let task = {
            let mut next_task_fut = pin!(next_task_fut);
            match exec.run_until_stalled(&mut next_task_fut) {
                Poll::Ready(Some(task)) => task,
                x => panic!("Expected next task to be sent after start, got {:?}", x),
            }
        };

        assert_eq!(task.peer_id, PeerId(1));
        assert_eq!(task.codec_config, expected_codec_config);

        assert!(task.is_started());
        assert!(stream.suspend().is_ok());
        assert!(!task.is_started());
        assert!(stream.start().is_ok());

        let next_task_fut = task_builder.next_task();
        // Task should be created here.
        let task = {
            let mut next_task_fut = pin!(next_task_fut);
            match exec.run_until_stalled(&mut next_task_fut) {
                Poll::Ready(Some(task)) => task,
                x => panic!("Expected next task to be sent after start, got {:?}", x),
            }
        };

        assert!(task.is_started());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn media_task_ending_ends_future(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let mut task_builder = TestMediaTaskBuilder::new();
        let mut stream = Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            task_builder.builder(),
        );
        let next_task_fut = task_builder.next_task();
        let peer_id = PeerId(1);
        let remote_id = 1_u8.try_into().expect("good id");

        let sbc_codec_cap = sbc_mediacodec_capability();
        let expected_codec_config =
            MediaCodecConfig::try_from(&sbc_codec_cap).expect("codec config");

        assert!(stream.configure(&peer_id, &remote_id, vec![]).is_err());
        assert!(stream.configure(&peer_id, &remote_id, vec![sbc_codec_cap]).is_ok());

        stream.endpoint_mut().establish().expect("establishment should start okay");
        let (transport, _remote) = create_test_channels(transport);
        let _ = stream.endpoint_mut().receive_channel(transport).expect("ready for a channel");

        let stream_finish_fut = stream.start().expect("start to succeed with a future");
        let mut stream_finish_fut = pin!(stream_finish_fut);

        let task = {
            let mut next_task_fut = pin!(next_task_fut);
            match exec.run_until_stalled(&mut next_task_fut) {
                Poll::Ready(Some(task)) => task,
                x => panic!("Expected next task to be sent after start, got {:?}", x),
            }
        };

        assert_eq!(task.peer_id, PeerId(1));
        assert_eq!(task.codec_config, expected_codec_config);

        // Does not need to be polled to be started.
        assert!(task.is_started());

        assert!(exec.run_until_stalled(&mut stream_finish_fut).is_pending());

        task.end_prematurely(Some(Ok(MediaTaskStatus::Stopped)));
        assert!(!task.is_started());

        // The future should be finished, since the task ended.
        match exec.run_until_stalled(&mut stream_finish_fut) {
            Poll::Ready(Ok(MediaTaskStatus::Stopped)) => {}
            x => panic!("Expected to get ready Ok from finish future, but got {x:?}"),
        };

        // Should still be able to suspend the stream.
        assert!(stream.suspend().is_ok());

        // And be able to restart it.
        let result_fut = stream.start().expect("start to succeed with a future");

        let next_task_fut = task_builder.next_task();
        let mut next_task_fut = pin!(next_task_fut);
        let task = match exec.run_until_stalled(&mut next_task_fut) {
            Poll::Ready(Some(task)) => task,
            x => panic!("Expected next task to be sent after restart, got {x:?}"),
        };

        assert!(task.is_started());

        // Dropping the result future shouldn't stop the media task.
        drop(result_fut);

        assert!(task.is_started());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn set_delay_correct_results_transmits_to_task(transport: Transport) {
        let mut _exec = fasync::TestExecutor::new();

        let mut task_builder = TestMediaTaskBuilder::new_delayable();
        let mut stream = Stream::build(
            make_sbc_endpoint(1, avdtp::EndpointType::Source),
            task_builder.builder(),
        );
        let peer_id = PeerId(1);
        let remote_id = 1_u8.try_into().expect("good id");

        let sbc_codec_cap = sbc_mediacodec_capability();

        let code = stream
            .set_delay(std::time::Duration::ZERO)
            .expect_err("before configure, can't set a delay");
        assert_eq!(ErrorCode::BadState, code);

        assert!(stream.configure(&peer_id, &remote_id, vec![]).is_err());
        assert!(stream.configure(&peer_id, &remote_id, vec![sbc_codec_cap]).is_ok());

        let delay_set = std::time::Duration::from_nanos(0xfeed);

        stream.set_delay(delay_set.clone()).expect("after configure, delay is fine");

        stream.endpoint_mut().establish().expect("establishment should start okay");
        let (transport, _remote) = create_test_channels(transport);
        let _ = stream.endpoint_mut().receive_channel(transport).expect("ready for a channel");
        let _stream_finish_fut = stream.start().expect("start to succeed with a future");

        let media_task = task_builder.expect_task();
        assert_eq!(delay_set, media_task.delay);
    }
}
