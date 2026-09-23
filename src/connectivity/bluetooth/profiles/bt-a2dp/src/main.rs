// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use anyhow::{Context as _, Error, format_err};
use bt_a2dp::codec::MediaCodecConfig;
use bt_a2dp::connected_peers::ConnectedPeers;
use bt_a2dp::peer::ControllerPool;
use bt_a2dp::permits::Permits;
use bt_a2dp::stream;
use bt_avdtp as avdtp;

use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_a2dp::{AudioModeRequest, AudioModeRequestStream, Role};
use fidl_fuchsia_bluetooth_avrcp as avrcp;

use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_component::BinderMarker;
use fidl_fuchsia_media::SessionAudioConsumerFactoryMarker;
use fidl_fuchsia_media_sessions2 as sessions2;
use fuchsia_async::{self as fasync, DurationExt};
use fuchsia_bluetooth::assigned_numbers::AssignedNumber;
use fuchsia_bluetooth::profile::{
    find_profile_descriptors, find_service_classes, profile_descriptor_to_assigned,
};
use fuchsia_bluetooth::types::{PeerId, Uuid};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::Inspect;
use futures::{Stream, StreamExt};
use log::{debug, error, info, trace, warn};
use profile_client::{ProfileClient, ProfileEvent};
use std::collections::HashSet;
use std::sync::Arc;

mod avrcp_relay;
mod config;
mod encoding;
mod latm;
mod media;
mod pcm_audio;
mod stream_controller;
mod volume_relay;

use config::A2dpConfiguration;
use media::player::Player;
use pcm_audio::PcmAudio;
use stream_controller::{PermitsManager, add_stream_controller_capability};

/// Make the SDP definition for the A2DP service.
pub(crate) fn make_profile_service_definition(service_uuid: Uuid) -> bredr::ServiceDefinition {
    bredr::ServiceDefinition {
        service_class_uuids: Some(vec![service_uuid.into()]),
        protocol_descriptor_list: Some(vec![
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![bredr::DataElement::Uint16(bredr::PSM_AVDTP)]),
                ..Default::default()
            },
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Avdtp),
                params: Some(vec![bredr::DataElement::Uint16(0x0103)]), // Indicate v1.3
                ..Default::default()
            },
        ]),
        profile_descriptors: Some(vec![bredr::ProfileDescriptor {
            profile_id: Some(bredr::ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(1),
            minor_version: Some(2),
            ..Default::default()
        }]),
        ..Default::default()
    }
}

// SDP Attribute ID for the Supported Features of A2DP.
// Defined in Assigned Numbers for SDP
// https://www.bluetooth.com/specifications/assigned-numbers/service-discovery
const ATTR_A2DP_SUPPORTED_FEATURES: u16 = 0x0311;

pub const DEFAULT_SAMPLE_RATE: u32 = 48000;
pub const DEFAULT_SESSION_ID: u64 = 0;

// Highest AAC bitrate we want to transmit
const MAX_BITRATE_AAC: u32 = 320000;

async fn streams_builder(
    metrics_logger: bt_metrics::MetricsLogger,
    config: &A2dpConfiguration,
) -> Result<stream::StreamsBuilder, Error> {
    // SBC is required to be playable if sink is enabled, check early before AAC.
    if config.enable_sink {
        let sbc_config = MediaCodecConfig::min_sbc();
        if let Err(e) = Player::test_playable(&sbc_config).await {
            warn!("Can't play required SBC audio: {e}");
            return Err(e);
        }
    }

    let aac_available = match config {
        A2dpConfiguration { enable_aac, .. } if !enable_aac => false,
        // If AAC is enabled and source-only presume the config is correct.
        A2dpConfiguration { enable_sink, .. } if !enable_sink => true,
        _ => {
            // Sink and AAC are enabled, test to see if we can play AAC audio.
            let aac_config = MediaCodecConfig::min_aac_sink();
            Player::test_playable(&aac_config).await.is_ok()
        }
    };

    let mut streams_builder = stream::StreamsBuilder::default();

    if config.enable_sink {
        let publisher =
            fuchsia_component::client::connect_to_protocol::<sessions2::PublisherMarker>()
                .context("Failed to connect to MediaSession interface")?;
        let audio_consumer_factory =
            fuchsia_component::client::connect_to_protocol::<SessionAudioConsumerFactoryMarker>()
                .context("Failed to connect to AudioConsumerFactory")?;
        let sink_builder = media::player_sink::Builder::new(
            metrics_logger.clone(),
            publisher,
            audio_consumer_factory,
            config.domain.clone(),
            aac_available,
        );
        streams_builder.add_builder(sink_builder);
    }

    let Some(source_type) = config.source else {
        return Ok(streams_builder);
    };

    use media::AudioSourceType::*;
    match source_type {
        AudioOut | BigBen => {
            let inband_source_builder =
                media::encoding_source::Builder::new(source_type, aac_available).await?;
            streams_builder.add_builder(inband_source_builder);
        }
        Offload => {
            let svc = fuchsia_component::client::connect_to_protocol::<
                fidl_fuchsia_audio_device::ProviderMarker,
            >()
            .context("Failed to connect to audio device Provider")?;
            let offload_source_builder = media::offload_source::Builder::new(svc)?;
            streams_builder.add_builder(offload_source_builder);
        }
    }
    Ok(streams_builder)
}

/// Establishes the signaling channel after an `initiator_delay`.
async fn connect_after_timeout(
    peer_id: PeerId,
    peers: Arc<ConnectedPeers>,
    channel_parameters: fidl_bt::ChannelParameters,
    initiator_delay: zx::MonotonicDuration,
) {
    trace!("waiting {}ms before connecting to peer {}.", initiator_delay.into_millis(), peer_id);
    fuchsia_async::Timer::new(initiator_delay.after_now()).await;

    trace!(peer_id:%; "trying to connect control channel");
    let connect_fut = peers.try_connect(peer_id, channel_parameters);
    let channel = match connect_fut.await {
        Err(e) => return warn!(peer_id:%, e:?; "Failed to connect control channel"),
        Ok(None) => return warn!(peer_id:%; "Control channel already connected"),
        Ok(Some(channel)) => channel,
    };

    info!(peer_id:%, mode:% = channel.channel_mode(), max_tx:% = channel.max_tx_size(); "Connected");
    if let Err(e) =
        peers.connected(peer_id, channel, Some(zx::MonotonicDuration::from_nanos(0))).await
    {
        warn!("Problem delivering connection to peer: {}", e);
    }
}

/// Returns the set of supported endpoint directions from a list of service classes.
fn find_endpoint_directions(service_classes: Vec<AssignedNumber>) -> HashSet<avdtp::EndpointType> {
    let mut directions = HashSet::new();
    if service_classes
        .iter()
        .any(|an| an.number == bredr::ServiceClassProfileIdentifier::AudioSource.into_primitive())
    {
        let _ = directions.insert(avdtp::EndpointType::Source);
    }
    if service_classes
        .iter()
        .any(|an| an.number == bredr::ServiceClassProfileIdentifier::AudioSink.into_primitive())
    {
        let _ = directions.insert(avdtp::EndpointType::Sink);
    }
    directions
}

/// Handles found services. Stores the found information and then spawns a task which will
/// assume initiator role after a delay.
fn handle_services_found(
    peer_id: &PeerId,
    attributes: &[bredr::Attribute],
    peers: Arc<ConnectedPeers>,
    channel_parameters: fidl_bt::ChannelParameters,
    initiator_delay: Option<zx::MonotonicDuration>,
) {
    let service_classes = find_service_classes(attributes);
    let service_names: Vec<&str> = service_classes.iter().map(|an| an.name).collect();
    let peer_preferred_directions = find_endpoint_directions(service_classes);
    let profiles = find_profile_descriptors(attributes).unwrap_or_default();
    let profile_names: Vec<String> = profiles
        .iter()
        .filter_map(|p| {
            profile_descriptor_to_assigned(p).map(|a| {
                format!(
                    "{} ({}.{})",
                    a.name,
                    p.major_version.map_or_else(|| "X".to_string(), |v| v.to_string()),
                    p.minor_version.map_or_else(|| "X".to_string(), |v| v.to_string())
                )
            })
        })
        .collect();
    info!(peer_id:%; "Found audio profile: {service_names:?}, profiles: {profile_names:?}");

    let Some(profile) = profiles.first() else {
        info!(peer_id:%; "Couldn't find profile in results, ignoring");
        return;
    };

    debug!(peer_id:%; "Marking found");
    peers.found(peer_id.clone(), profile.clone(), peer_preferred_directions);

    if let Some(initiator_delay) = initiator_delay {
        fasync::Task::local(connect_after_timeout(
            peer_id.clone(),
            peers,
            channel_parameters,
            initiator_delay,
        ))
        .detach();
    }
}

/// Handles role change requests from serving AudioMode
fn handle_audio_mode_connection(peers: Arc<ConnectedPeers>, mut stream: AudioModeRequestStream) {
    fasync::Task::spawn(async move {
        info!("AudioMode Client connected");
        while let Some(request) = stream.next().await {
            match request {
                Err(e) => info!("AudioMode client error: {e}"),
                Ok(AudioModeRequest::SetRole { role, responder }) => {
                    // We want to be `role` so we prefer to start streams of the opposite direction.
                    let direction = match role {
                        Role::Source => avdtp::EndpointType::Sink,
                        Role::Sink => avdtp::EndpointType::Source,
                    };
                    info!("Setting AudioMode to {role:?}");
                    peers.set_preferred_peer_direction(direction);
                    if let Err(e) = responder.send() {
                        warn!("Failed to respond to mode request: {e}");
                    }
                }
            }
        }
    })
    .detach();
}

fn setup_profiles(
    proxy: bredr::ProfileProxy,
    config: &config::A2dpConfiguration,
) -> profile_client::Result<ProfileClient> {
    let mut service_defs = Vec::new();
    if config.source.is_some() {
        let source_uuid =
            Uuid::new16(bredr::ServiceClassProfileIdentifier::AudioSource.into_primitive());
        service_defs.push(make_profile_service_definition(source_uuid));
    }

    if config.enable_sink {
        let sink_uuid =
            Uuid::new16(bredr::ServiceClassProfileIdentifier::AudioSink.into_primitive());
        service_defs.push(make_profile_service_definition(sink_uuid));
    }

    let mut profile = ProfileClient::advertise(proxy, service_defs, config.channel_parameters())?;

    let attr_ids = vec![
        bredr::ATTR_PROTOCOL_DESCRIPTOR_LIST,
        bredr::ATTR_SERVICE_CLASS_ID_LIST,
        bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST,
        ATTR_A2DP_SUPPORTED_FEATURES,
    ];

    if config.source.is_some() {
        profile
            .add_search(bredr::ServiceClassProfileIdentifier::AudioSink, Some(attr_ids.clone()))?;
    }

    if config.enable_sink {
        profile.add_search(bredr::ServiceClassProfileIdentifier::AudioSource, Some(attr_ids))?;
    }

    Ok(profile)
}

/// The number of allowed active streams across the whole profile.
/// If a peer attempts to start an audio stream and there are already this many active, it will
/// be suspended immediately.
const ACTIVE_STREAM_LIMIT: usize = 1;

#[fuchsia::main(logging_tags = ["bt-a2dp"])]
async fn main() -> Result<(), Error> {
    let config = A2dpConfiguration::load_default()?;
    let init_delay_ms = config.initiator_delay.into_millis();

    let initiator_delay = (init_delay_ms != 0).then_some(config.initiator_delay);

    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let controller_pool = Arc::new(ControllerPool::new());

    let mut fs = ServiceFs::new();

    let inspect = inspect::Inspector::default();
    let _inspect_server_task =
        inspect_runtime::publish(&inspect, inspect_runtime::PublishOptions::default());

    // The absolute volume relay is only needed if A2DP Sink is requested.
    let _abs_vol_relay = config.enable_sink.then(|| {
        volume_relay::VolumeRelay::start()
            .or_else(|e| {
                warn!("Failed to start AbsoluteVolume Relay: {e:?}");
                Err(e)
            })
            .ok()
    });

    // Set up the metrics logger.
    let metrics_logger = bt_metrics::MetricsLogger::new();

    let stream_builder = streams_builder(metrics_logger.clone(), &config).await?;
    let profile_svc = fuchsia_component::client::connect_to_protocol::<bredr::ProfileMarker>()
        .context("Failed to connect to Bluetooth Profile service")?;

    let permits = Permits::new(ACTIVE_STREAM_LIMIT);

    let avrcp = config
        .source
        .is_some()
        .then(|| {
            fuchsia_component::client::connect_to_protocol::<avrcp::PeerManagerMarker>()
                .map_err(|e| warn!("Failed to connect to AVRCP: {e:?}"))
                .ok()
        })
        .flatten();

    let mut peers = ConnectedPeers::new(
        stream_builder,
        permits.clone(),
        profile_svc.clone(),
        avrcp,
        metrics_logger,
    );
    if let Err(e) = peers.iattach(&inspect.root(), "connected") {
        warn!("Failed to attach to inspect: {e:?}");
    }

    let peers_connected_stream = peers.connected_stream();
    let _controller_pool_connected_task = fasync::Task::spawn({
        let pool = controller_pool.clone();
        peers_connected_stream.map(move |p| pool.peer_connected(p)).collect::<()>()
    });

    // The AVRCP Target component is needed if it is requested and A2DP Source is requested.
    #[allow(clippy::collection_is_never_read)]
    let mut _avrcp_target = None;
    if config.source.is_some() && config.enable_avrcp_target {
        match fuchsia_component::client::connect_to_protocol::<BinderMarker>() {
            Err(e) => warn!("Couldn't start AVRCP target: {e}"),
            Ok(tg) => {
                _avrcp_target = Some(tg);
            }
        }
    }

    let peers = Arc::new(peers);

    // `bt-a2dp` provides the `avdtp.test.PeerManager`, `a2dp.AudioMode`, and
    // `internal.a2dp.Controller` capabilities.
    let _ =
        fs.dir("svc").add_fidl_service(move |s| controller_pool.connected(s)).add_fidl_service({
            let peers = peers.clone();
            move |s| handle_audio_mode_connection(peers.clone(), s)
        });
    add_stream_controller_capability(&mut fs, PermitsManager::from(permits));

    if let Err(e) = fs.take_and_serve_directory_handle() {
        warn!("Unable to serve service directory: {e}");
    }

    let _servicefs_task = fasync::Task::spawn(fs.collect::<()>());

    let profile = match setup_profiles(profile_svc.clone(), &config) {
        Err(e) => {
            let err = format!("Failed to setup profiles: {e:?}");
            error!("{err}");
            return Err(format_err!("{err}"));
        }
        Ok(profile) => profile,
    };

    handle_profile_events(profile, peers, config.channel_parameters(), initiator_delay).await
}

async fn handle_profile_events(
    mut profile: impl Stream<Item = profile_client::Result<ProfileEvent>> + Unpin,
    peers: Arc<ConnectedPeers>,
    channel_parameters: fidl_bt::ChannelParameters,
    initiator_delay: Option<zx::MonotonicDuration>,
) -> Result<(), Error> {
    while let Some(item) = profile.next().await {
        let Ok(evt) = item else {
            return Err(format_err!("Profile client error: {:?}", item.err()));
        };
        let peer_id = evt.peer_id();
        match evt {
            ProfileEvent::PeerConnected { channel, .. } => {
                info!(peer_id:%, mode:% = channel.channel_mode(), max_tx:% = channel.max_tx_size(); "Incoming connection");
                // Connected, initiate after the delay if not streaming.
                if let Err(e) = peers.connected(peer_id, channel, initiator_delay).await {
                    warn!("Problem accepting peer connection: {e}");
                }
            }
            ProfileEvent::SearchResult { attributes, .. } => {
                handle_services_found(
                    &peer_id,
                    &attributes,
                    peers.clone(),
                    channel_parameters.clone(),
                    initiator_delay,
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth_a2dp as a2dp;
    use fidl_fuchsia_bluetooth_bredr::{ProfileRequest, ProfileRequestStream};
    use futures::task::Poll;
    use test_case::test_case;

    use crate::config::DEFAULT_INITIATOR_DELAY;
    use crate::media::AudioSourceType;

    fn run_to_stalled(exec: &mut fasync::TestExecutor) {
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
    }

    fn setup_connected_peers() -> (Arc<ConnectedPeers>, ProfileRequestStream) {
        let (proxy, stream) = create_proxy_and_stream::<bredr::ProfileMarker>();
        let peers = Arc::new(ConnectedPeers::new(
            stream::StreamsBuilder::default(),
            Permits::new(1),
            proxy,
            None,
            bt_metrics::MetricsLogger::default(),
        ));
        (peers, stream)
    }

    #[cfg(not(feature = "test_encoding"))]
    #[fuchsia::test]
    /// build_local_streams should fail because it can't start the SBC decoder, because
    /// MediaPlayer isn't available in the test environment.
    fn test_sbc_unavailable_error() {
        let mut exec = fasync::TestExecutor::new();
        let config =
            A2dpConfiguration { source: Some(AudioSourceType::BigBen), ..Default::default() };
        let mut streams_fut =
            Box::pin(streams_builder(bt_metrics::MetricsLogger::default(), &config));

        let streams_builder = exec.run_singlethreaded(&mut streams_fut);

        assert!(
            streams_builder.is_err(),
            "Stream building should fail when it can't play SBC / encode SBC"
        );
    }

    #[cfg(feature = "test_encoding")]
    #[fuchsia::test]
    /// build local_streams should not include the AAC streams
    fn test_aac_switch() {
        let mut exec = fasync::TestExecutor::new();
        let mut config = A2dpConfiguration {
            source: Some(AudioSourceType::BigBen),
            enable_sink: false,
            ..Default::default()
        };
        let mut builder_fut =
            Box::pin(streams_builder(bt_metrics::MetricsLogger::default(), &config));

        let builder_res = exec.run_singlethreaded(&mut builder_fut);

        let builder = builder_res.expect("should generate streams builder");
        let mut peer_streams_fut = Box::pin(builder.peer_streams(&PeerId(1), None));

        let peer_streams = exec.run_singlethreaded(&mut peer_streams_fut).expect("Should succeed");

        assert_eq!(peer_streams.information().len(), 2, "Source AAC & SBC should be available");

        drop(peer_streams_fut);
        drop(builder_fut);
        drop(peer_streams);

        config.enable_aac = false;

        let mut builder_fut =
            Box::pin(streams_builder(bt_metrics::MetricsLogger::default(), &config));

        let builder_res = exec.run_singlethreaded(&mut builder_fut);

        let builder = builder_res.expect("should generate streams builder");
        let mut peer_streams_fut = Box::pin(builder.peer_streams(&PeerId(1), None));

        let peer_streams = exec.run_singlethreaded(&mut peer_streams_fut).expect("Should succeed");

        assert_eq!(peer_streams.information().len(), 1, "Source SBC only should be available");
    }

    /// Set the time to `time`, and then wake any expired timers and run until the main loop stalls.
    fn forward_time_to(exec: &mut fasync::TestExecutor, time: fasync::MonotonicInstant) {
        exec.set_fake_time(time);
        let _ = exec.wake_expired_timers();
        run_to_stalled(exec);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[fuchsia::test]
    /// Tests that A2DP sink assumes the initiator role when a peer is found, but
    /// not connected, and the timeout completes.
    fn wait_to_initiate_success_with_no_connected_peer(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (peers, mut prof_stream) = setup_connected_peers();
        // Initialize context to a fixed point in time.
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1000000000));
        let peer_id = PeerId(1);

        // Simulate getting the service found event.
        let attributes = vec![bredr::Attribute {
            id: Some(bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
            element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                bredr::DataElement::Sequence(vec![
                    Some(Box::new(
                        Uuid::from(bredr::ServiceClassProfileIdentifier::AudioSource).into(),
                    )),
                    Some(Box::new(bredr::DataElement::Uint16(0x0103))), // Version 1.3
                ]),
            ))])),
            ..Default::default()
        }];
        handle_services_found(
            &peer_id,
            &attributes,
            peers.clone(),
            fidl_bt::ChannelParameters {
                channel_mode: Some(fidl_bt::ChannelMode::Basic),
                max_rx_packet_size: Some(crate::config::MAX_RX_SDU_SIZE),
                ..Default::default()
            },
            Some(DEFAULT_INITIATOR_DELAY),
        );

        run_to_stalled(&mut exec);

        // At this point, a remote peer was found, but hasn't connected yet. There
        // should be no entry for it.
        assert!(!peers.is_connected(&peer_id));

        // Fast forward time by 5 seconds. In this time, the remote peer has not
        // connected.
        forward_time_to(
            &mut exec,
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5)),
        );

        // After fast forwarding time, expect and handle the `connect` request
        // because A2DP-sink should be initiating.
        let (_test, local_channel) = create_test_channels(transport);
        let request = exec.run_until_stalled(&mut prof_stream.next());
        match request {
            Poll::Ready(Some(Ok(ProfileRequest::Connect {
                peer_id,
                responder,
                connection,
                ..
            }))) => {
                assert_eq!(PeerId(1), peer_id.into());
                match connection {
                    bredr::ConnectParameters::L2cap(params) => assert_eq!(
                        Some(crate::config::MAX_RX_SDU_SIZE),
                        params.parameters.unwrap().max_rx_packet_size
                    ),
                    x => panic!("Expected L2cap connection, got {:?}", x),
                };
                let channel = local_channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("responder sends");
            }
            x => panic!("Should have sent a connect request, but got {:?}", x),
        };
        run_to_stalled(&mut exec);

        // The remote peer did not connect to us, A2DP Sink should initiate a connection
        // and insert into `peers`.
        assert!(peers.is_connected(&peer_id));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    /// Tests that A2DP sink does not assume the initiator role when a peer connects
    /// before `INITIATOR_DELAY` timeout completes.
    fn wait_to_initiate_returns_early_with_connected_peer(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (peers, mut prof_stream) = setup_connected_peers();
        // Initialize context to a fixed point in time.
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(1000000000));
        let peer_id = PeerId(1);

        // Simulate getting the service found event.
        let attributes = vec![bredr::Attribute {
            id: Some(bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
            element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                bredr::DataElement::Sequence(vec![
                    Some(Box::new(
                        Uuid::from(bredr::ServiceClassProfileIdentifier::AudioSource).into(),
                    )),
                    Some(Box::new(bredr::DataElement::Uint16(0x0103))), // Version 1.3
                ]),
            ))])),
            ..Default::default()
        }];
        handle_services_found(
            &peer_id,
            &attributes,
            peers.clone(),
            fidl_bt::ChannelParameters::default(),
            Some(DEFAULT_INITIATOR_DELAY),
        );

        // At this point, a remote peer was found, but hasn't connected yet. There
        // should be no entry for it.
        assert!(!peers.is_connected(&peer_id));

        // Fast forward time by .5 seconds. The threshold is 1 second, so the timer
        // to initiate connections has not been triggered.
        forward_time_to(
            &mut exec,
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(500)),
        );

        // A peer connects before the timeout.
        let (_remote, signaling) = create_test_channels(transport);
        let mut connected_fut = std::pin::pin!(peers.connected(peer_id.clone(), signaling, None));
        let _detachable_peer =
            exec.run_until_stalled(&mut connected_fut).expect("ready").expect("okay");
        run_to_stalled(&mut exec);

        // The remote peer connected to us, and should be in the map.
        assert!(peers.is_connected(&peer_id));

        // Fast forward time by 4.5 seconds. Ensure no outbound connection is initiated
        // by us, since the remote peer has assumed the INT role.
        forward_time_to(
            &mut exec,
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(4500)),
        );

        let request = exec.run_until_stalled(&mut prof_stream.next());
        match request {
            Poll::Ready(x) => panic!("There should be no l2cap connection requests: {:?}", x),
            Poll::Pending => {}
        };
        run_to_stalled(&mut exec);
    }

    #[fuchsia::test]
    fn test_audio_mode_connection() {
        let mut exec = fasync::TestExecutor::new();
        let (peers, _profile_stream) = setup_connected_peers();

        let (proxy, stream) = create_proxy_and_stream::<a2dp::AudioModeMarker>();

        handle_audio_mode_connection(peers.clone(), stream);

        exec.run_singlethreaded(proxy.set_role(a2dp::Role::Sink)).expect("set role response");

        assert_eq!(avdtp::EndpointType::Source, peers.preferred_peer_direction());

        exec.run_singlethreaded(proxy.set_role(a2dp::Role::Source)).expect("set role response");

        assert_eq!(avdtp::EndpointType::Sink, peers.preferred_peer_direction());
    }

    #[fuchsia::test]
    fn find_endpoint_directions_returns_expected_direction() {
        let empty = Vec::new();
        assert_eq!(find_endpoint_directions(empty), HashSet::new());

        let no_a2dp_attributes =
            vec![AssignedNumber { number: 0x1234, abbreviation: None, name: "FooBar" }];
        assert_eq!(find_endpoint_directions(no_a2dp_attributes), HashSet::new());

        let sink_attribute = AssignedNumber {
            number: bredr::ServiceClassProfileIdentifier::AudioSink.into_primitive(),
            abbreviation: None,
            name: "AudioSink",
        };
        let source_attribute = AssignedNumber {
            number: bredr::ServiceClassProfileIdentifier::AudioSource.into_primitive(),
            abbreviation: None,
            name: "AudioSource",
        };

        let only_sink = vec![sink_attribute.clone()];
        let expected_directions = HashSet::from_iter(vec![avdtp::EndpointType::Sink].into_iter());
        assert_eq!(find_endpoint_directions(only_sink), expected_directions);

        let only_source = vec![source_attribute.clone()];
        let expected_directions = HashSet::from_iter(vec![avdtp::EndpointType::Source].into_iter());
        assert_eq!(find_endpoint_directions(only_source), expected_directions);

        let both = vec![sink_attribute, source_attribute];
        let expected_directions = HashSet::from_iter(
            vec![avdtp::EndpointType::Sink, avdtp::EndpointType::Source].into_iter(),
        );
        assert_eq!(find_endpoint_directions(both), expected_directions);
    }
}
