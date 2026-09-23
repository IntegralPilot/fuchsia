// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, format_err};
use fuchsia_bluetooth::types::{PeerId, Uuid};
use futures::stream::BoxStream;
use std::collections::HashSet;
use std::str::FromStr;
use thiserror::Error;
use {
    fidl_fuchsia_audio_device as audio_device, fidl_fuchsia_bluetooth_bredr as bredr,
    fidl_fuchsia_media as media,
};

use crate::codec_id::CodecId;
use crate::sco;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Parameters aren't supported {:?}", .source)]
    UnsupportedParameters { source: anyhow::Error },
    #[error("Audio is already started")]
    AlreadyStarted,
    #[error("AudioCore Error: {:?}", .source)]
    AudioCore { source: anyhow::Error },
    #[error("FIDL Error: {:?}", .0)]
    Fidl(#[from] fidl::Error),
    #[error("Audio is not started")]
    NotStarted,
    #[error("Could not find suitable devices")]
    DiscoveryFailed,
    #[error("Operation is in progress: {}", .description)]
    InProgress { description: String },
}

impl Error {
    fn audio_core(e: anyhow::Error) -> Self {
        Self::AudioCore { source: e }
    }
}

mod codec;
pub use codec::CodecControl;

mod dai;
pub use dai::DaiControl;

mod inband;
pub use inband::InbandControl;

mod partial_offload;
pub use partial_offload::PartialOffloadControl;

mod test;
pub use test::TestControl;

const DEVICE_NAME: &'static str = "Bluetooth HFP";

// Used to build audio device IDs for peers
const HF_INPUT_UUID: Uuid =
    Uuid::new16(bredr::ServiceClassProfileIdentifier::Handsfree.into_primitive());
const HF_OUTPUT_UUID: Uuid =
    Uuid::new16(bredr::ServiceClassProfileIdentifier::HandsfreeAudioGateway.into_primitive());

#[derive(Debug)]
pub enum ControlEvent {
    /// Request from Control to start the audio for a connected Peer.
    /// Control::start() or Control::failed_request() should be called after this event
    /// has been received; incompatible calls may return `Error::InProgress`.
    RequestStart { id: PeerId },
    /// Request from the Control to stop the audio to a connected Peer.
    /// Any calls for which audio is currently routed to the HF for this peer will be transferred
    /// to the AG.
    /// Control::stop() should be called after this event has been received; incompatible
    /// calls made in this state may return `Error::InProgress`
    /// Note that Control::failed_request() may not be called for this event (audio can
    /// always be stopped)
    RequestStop { id: PeerId },
    /// Event produced when an audio path has been started and audio is flowing to/from the peer.
    Started { id: PeerId },
    /// Event produced when the audio path has been stopped and audio is not flowing to/from the
    /// peer.
    /// This event can be spontaeously produced by the Control implementation to indicate an
    /// error in the audio path (either during or after a requested start).
    Stopped { id: PeerId, error: Option<Error> },
}

impl ControlEvent {
    pub fn id(&self) -> PeerId {
        match self {
            ControlEvent::RequestStart { id } => *id,
            ControlEvent::RequestStop { id } => *id,
            ControlEvent::Started { id } => *id,
            ControlEvent::Stopped { id, error: _ } => *id,
        }
    }
}

pub trait Control: Send {
    /// Send to indicate when connected to a peer. `supported_codecs` indicates the set of codecs which are
    /// communicated from the peer.  Depending on the audio control implementation,
    /// this may add a (stopped) media device.  Audio control implementations can request audio be started
    /// for peers that are connected.
    fn connect(&mut self, id: PeerId, supported_codecs: &[CodecId]);

    /// Send to indicate that a peer has been disconnected.  This shall tear down any audio path
    /// set up for the peer and send a `ControlEvent::Stopped` for each.  This shall be idempotent
    /// (calling disconnect on a disconnected PeerId does nothing)
    fn disconnect(&mut self, id: PeerId);

    /// Request to start sending audio to the peer.  If the request succeeds `Ok(())` will be
    /// returned, but audio may not be started until a `ControlEvent::Started` event is
    /// produced in the events.
    fn start(
        &mut self,
        id: PeerId,
        connection: sco::Connection,
        codec: CodecId,
    ) -> Result<(), Error>;

    /// Request to stop the audio to a peer.
    /// If the Audio is not started, an Err(Error::NotStarted) will be returned.
    /// If the requests succeeds `Ok(())` will be returned but audio may not be stopped until a
    /// `ControlEvent::Stopped` is produced in the events.
    fn stop(&mut self, id: PeerId) -> Result<(), Error>;

    /// Get a stream of the events produced by this audio control.
    /// May panic if the event stream has already been taken.
    fn take_events(&self) -> BoxStream<'static, ControlEvent>;

    /// Respond with failure to a request from the event stream.
    /// `request` should be the request that failed.  If a request was not made by this audio
    /// control the failure shall be ignored.
    fn failed_request(&self, request: ControlEvent, error: Error);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OffloadType {
    Dai,
    Codec,
}

impl FromStr for OffloadType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dai" => Ok(OffloadType::Dai),
            "codec" => Ok(OffloadType::Codec),
            _ => Err(format_err!("Invalid offload type: {}", s)),
        }
    }
}

pub async fn setup_audio(
    offload_type: OffloadType,
    controller_codecs: HashSet<CodecId>,
) -> Result<Box<dyn Control>, anyhow::Error> {
    match offload_type {
        OffloadType::Dai => {
            let audio_proxy = fuchsia_component::client::connect_to_protocol::<
                media::AudioDeviceEnumeratorMarker,
            >()
            .with_context(|| format!("Error connecting to audio_core"))?;
            Ok(Box::new(
                PartialOffloadControl::setup_audio_core(audio_proxy, controller_codecs).await?,
            ))
        }
        OffloadType::Codec => {
            let provider =
                fuchsia_component::client::connect_to_protocol::<audio_device::ProviderMarker>()
                    .with_context(|| format!("Error connecting to audio_device_registry"))?;
            let codec = CodecControl::new(provider);
            Ok(Box::new(codec))
        }
    }
}
