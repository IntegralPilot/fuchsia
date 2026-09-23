// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements the Published Audio Capabilities Service server role.
//!
//! Use the `ServerBuilder` to define a new `Server` instance with the specified
//! characteristics. The server isn't published to GATT until
//! `Server::publish` method is called.
//! Once the `Server` is published, poll on it to receive events from the
//! `Server`, which are created as it processes incoming client requests.
//!
//! For example:
//!
//! // Set up a GATT Server which implements `bt_gatt::ServerTypes::Server`.
//! let gatt_server = ...;
//! // Define supported and available audio contexts for this PACS.
//! let supported = AudioContexts::new(...);
//! let available = AudioContexts::new(...);
//! let pacs_server = ServerBuilder::new()
//!         .with_sources(...)
//!         .with_sinks(...)
//!         .build(supported, available)?;
//!
//! // Publish the server.
//! pacs_server.publish(gatt_server).expect("publishes fine");
//! // Process events from the PACS server.
//! while let Some(event) = pacs_server.next().await {
//!     // Do something with `event`
//! }

use bt_common::generic_audio::ContextType;
use bt_gatt::server::LocalService;
use bt_gatt::server::{ReadResponder, ServiceDefinition, WriteResponder};
use bt_gatt::types::{GattError, Handle};
use bt_gatt::Server as _;

use futures::task::{Poll, Waker};
use futures::{Future, Stream};
use pin_project::pin_project;
use std::collections::HashMap;
use thiserror::Error;

use crate::{
    AudioLocations, AvailableAudioContexts, AvailableContexts, PacRecord, SinkAudioLocations,
    SourceAudioLocations, SupportedAudioContexts,
};

pub(crate) mod types;
use crate::server::types::*;

#[pin_project(project = LocalServiceProj)]
enum LocalServiceState<T: bt_gatt::ServerTypes> {
    NotPublished {
        waker: Option<Waker>,
    },
    Preparing {
        #[pin]
        fut: T::LocalServiceFut,
    },
    Published {
        service: T::LocalService,
        #[pin]
        events: T::ServiceEventStream,
    },
    Terminated,
}

impl<T: bt_gatt::ServerTypes> Default for LocalServiceState<T> {
    fn default() -> Self {
        Self::NotPublished { waker: None }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Service is already published")]
    AlreadyPublished,
    #[error("Issue publishing service: {0}")]
    PublishError(#[from] bt_gatt::types::Error),
    #[error("Service should support at least one of Sink or Source PAC characteristics")]
    MissingPac,
    #[error("Available audio contexts are not supported: {0:?}")]
    UnsupportedAudioContexts(Vec<ContextType>),
}

impl<T: bt_gatt::ServerTypes> Stream for LocalServiceState<T> {
    type Item = Result<bt_gatt::server::ServiceEvent<T>, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // SAFETY:
        //  - Wakers are Unpin
        //  - We re-pin the structurally pinned futures in Preparing and Published
        //    (service is untouched)
        //  - Terminated is empty
        loop {
            match self.as_mut().project() {
                LocalServiceProj::Terminated => return Poll::Ready(None),
                LocalServiceProj::NotPublished { .. } => {
                    self.as_mut()
                        .set(LocalServiceState::NotPublished { waker: Some(cx.waker().clone()) });
                    return Poll::Pending;
                }
                LocalServiceProj::Preparing { fut } => {
                    let service_result = futures::ready!(fut.poll(cx));
                    let Ok(service) = service_result else {
                        return Poll::Ready(Some(Err(Error::PublishError(
                            service_result.err().unwrap(),
                        ))));
                    };
                    let events = service.publish();
                    self.as_mut().set(LocalServiceState::Published { service, events });
                    continue;
                }
                LocalServiceProj::Published { service: _, events } => {
                    let item = futures::ready!(events.poll_next(cx));
                    let Some(gatt_result) = item else {
                        self.as_mut().set(LocalServiceState::Terminated);
                        return Poll::Ready(None);
                    };
                    let Ok(event) = gatt_result else {
                        self.as_mut().set(LocalServiceState::Terminated);
                        return Poll::Ready(Some(Err(Error::PublishError(
                            gatt_result.err().unwrap(),
                        ))));
                    };
                    return Poll::Ready(Some(Ok(event)));
                }
            }
        }
    }
}

impl<T: bt_gatt::ServerTypes> LocalServiceState<T> {
    fn is_published(&self) -> bool {
        if let LocalServiceState::NotPublished { .. } = self {
            false
        } else {
            true
        }
    }
}

#[derive(Default)]
pub struct ServerBuilder {
    source_pacs: Vec<Vec<PacRecord>>,
    source_audio_locations: Option<AudioLocations>,
    sink_pacs: Vec<Vec<PacRecord>>,
    sink_audio_locations: Option<AudioLocations>,
}

impl ServerBuilder {
    pub fn new() -> ServerBuilder {
        ServerBuilder::default()
    }

    /// Adds a source PAC characteristic to the builder.
    /// Each call adds a new characteristic.
    /// `capabilities` represents the records for a single PAC characteristic.
    /// If `capabilities` is empty, it will be ignored.
    pub fn add_source(mut self, capabilities: Vec<PacRecord>) -> Self {
        if !capabilities.is_empty() {
            self.source_pacs.push(capabilities);
        }
        self
    }

    /// Sets the audio locations for the source.
    /// This corresponds to a single Source Audio Locations characteristic.
    pub fn set_source_locations(mut self, audio_locations: AudioLocations) -> Self {
        self.source_audio_locations = Some(audio_locations);
        self
    }

    /// Adds a sink PAC characteristic to the builder.
    /// Each call adds a new characteristic.
    /// `capabilities` represents the records for a single PAC characteristic.
    /// If `capabilities` is empty, it will be ignored.
    pub fn add_sink(mut self, capabilities: Vec<PacRecord>) -> Self {
        if !capabilities.is_empty() {
            self.sink_pacs.push(capabilities);
        }
        self
    }

    /// Sets the audio locations for the sink.
    /// This corresponds to a single Sink Audio Locations characteristic.
    pub fn set_sink_locations(mut self, audio_locations: AudioLocations) -> Self {
        self.sink_audio_locations = Some(audio_locations);
        self
    }

    fn verify_characteristics(
        &self,
        supported: &AudioContexts,
        available: &AudioContexts,
    ) -> Result<(), Error> {
        // If the corresponding bit in the supported audio contexts is
        // not set to 0b1, we shall not set a bit to 0b1 in the
        // available audio contexts. See PACS v1.0.1 section 3.5.1.
        let diff: Vec<ContextType> = available.sink.difference(&supported.sink).cloned().collect();
        if diff.len() != 0 {
            return Err(Error::UnsupportedAudioContexts(diff));
        }
        let diff: Vec<ContextType> =
            available.source.difference(&supported.source).cloned().collect();
        if diff.len() != 0 {
            return Err(Error::UnsupportedAudioContexts(diff));
        }

        // PACS server must have at least one Sink or Source PACS record.
        if self.source_pacs.len() == 0 && self.sink_pacs.len() == 0 {
            return Err(Error::MissingPac);
        }
        Ok(())
    }

    /// Builds a server after verifying all the defined characteristics
    /// for this server (see PACS v1.0.1 section 3 for details).
    pub fn build<T>(
        mut self,
        mut supported: AudioContexts,
        available: AudioContexts,
    ) -> Result<Server<T>, Error>
    where
        T: bt_gatt::ServerTypes,
    {
        let _ = self.verify_characteristics(&supported, &available)?;

        let mut service_def = ServiceDefinition::new(
            bt_gatt::server::ServiceId::new(1),
            crate::PACS_UUID,
            bt_gatt::types::ServiceKind::Primary,
        );

        let supported = SupportedAudioContexts {
            handle: SUPPORTED_AUDIO_CONTEXTS_HANDLE,
            sink: supported.sink.drain().collect(),
            source: supported.source.drain().collect(),
        };
        let _ = service_def.add_characteristic((&supported).into());

        let available = AvailableAudioContexts {
            handle: AVAILABLE_AUDIO_CONTEXTS_HANDLE,
            sink: (&available.sink).into(),
            source: (&available.source).into(),
        };
        let _ = service_def.add_characteristic((&available).into());

        let mut next_handle_iter = (HANDLE_OFFSET..).map(|x| Handle(x));
        let mut audio_capabilities = HashMap::new();

        // Sink audio locations characteristic may exist iff it's defined
        // and there are valid sink PAC characteristics.
        let sink_audio_locations = match self.sink_audio_locations.take() {
            Some(locations) if self.sink_pacs.len() > 0 => {
                let sink =
                    SinkAudioLocations { handle: next_handle_iter.next().unwrap(), locations };
                let _ = service_def.add_characteristic((&sink).into());
                Some(sink)
            }
            _ => None,
        };
        for capabilities in self.sink_pacs.drain(..) {
            let handle = next_handle_iter.next().unwrap();
            let pac = PublishedAudioCapability::new_sink(handle, capabilities);
            let _ = service_def.add_characteristic((&pac).into());
            audio_capabilities.insert(handle, pac);
        }

        // Source audio locations characteristic may exist iff it's defined
        // and there are valid source PAC characteristics.
        let source_audio_locations = match self.source_audio_locations.take() {
            Some(locations) if self.source_pacs.len() > 0 => {
                let source =
                    SourceAudioLocations { handle: next_handle_iter.next().unwrap(), locations };
                let _ = service_def.add_characteristic((&source).into());
                Some(source)
            }
            _ => None,
        };
        for capabilities in self.source_pacs.drain(..) {
            let handle = next_handle_iter.next().unwrap();
            let pac = PublishedAudioCapability::new_source(handle, capabilities);
            let _ = service_def.add_characteristic((&pac).into());
            audio_capabilities.insert(handle, pac);
        }

        let server = Server {
            service_def,
            local_service: Default::default(),
            published_audio_capabilities: audio_capabilities,
            source_audio_locations,
            sink_audio_locations,
            available_audio_contexts: available,
            supported_audio_contexts: supported,
        };
        Ok(server)
    }
}

#[pin_project]
pub struct Server<T: bt_gatt::ServerTypes> {
    service_def: ServiceDefinition,
    #[pin]
    local_service: LocalServiceState<T>,
    published_audio_capabilities: HashMap<Handle, PublishedAudioCapability>,
    source_audio_locations: Option<SourceAudioLocations>,
    sink_audio_locations: Option<SinkAudioLocations>,
    available_audio_contexts: AvailableAudioContexts,
    supported_audio_contexts: SupportedAudioContexts,
}

impl<T: bt_gatt::ServerTypes> Server<T> {
    pub fn publish(&mut self, server: T::Server) -> Result<(), Error> {
        if self.local_service.is_published() {
            return Err(Error::AlreadyPublished);
        }

        let LocalServiceState::NotPublished { waker } = std::mem::replace(
            &mut self.local_service,
            LocalServiceState::Preparing { fut: server.prepare(self.service_def.clone()) },
        ) else {
            unreachable!();
        };
        waker.map(Waker::wake);
        Ok(())
    }

    fn is_source_locations_handle(&self, handle: Handle) -> bool {
        self.source_audio_locations.as_ref().map_or(false, |locations| locations.handle == handle)
    }

    fn is_sink_locations_handle(&self, handle: Handle) -> bool {
        self.sink_audio_locations.as_ref().map_or(false, |locations| locations.handle == handle)
    }

    /// Updates available audio contexts by overwritting the previously
    /// set available audio contexts value. If a context type is not
    /// part of the supported audio contexts, it will be ignored and
    /// the resulting available audio contexts would not contain that
    /// particular type.
    /// Returns the reference to the resulting available audio context.
    pub fn update_available(&mut self, available: AudioContexts) -> &AvailableAudioContexts {
        let supported = &self.supported_audio_contexts;
        let new_sink =
            AvailableContexts::from_iter(available.sink.intersection(&supported.sink).cloned());
        let new_source =
            AvailableContexts::from_iter(available.source.intersection(&supported.source).cloned());

        let changed = self.available_audio_contexts.sink != new_sink
            || self.available_audio_contexts.source != new_source;

        self.available_audio_contexts.sink = new_sink;
        self.available_audio_contexts.source = new_source;

        if !changed {
            return &self.available_audio_contexts;
        }
        if let LocalServiceState::Published { service, .. } = &self.local_service {
            service.notify(
                &AVAILABLE_AUDIO_CONTEXTS_HANDLE,
                &self.available_audio_contexts.into_char_value(),
                &[],
            );
        }
        &self.available_audio_contexts
    }
}

impl<T: bt_gatt::ServerTypes> Stream for Server<T> {
    type Item = Result<(), Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        loop {
            let mut this = self.as_mut().project();
            let gatt_event = match futures::ready!(this.local_service.as_mut().poll_next(cx)) {
                None => return Poll::Ready(None),
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                Some(Ok(event)) => event,
            };
            use bt_gatt::server::ServiceEvent::*;
            match gatt_event {
                Read { handle, offset, responder, .. } => {
                    let offset = offset as usize;
                    let value = match handle {
                        x if x == AVAILABLE_AUDIO_CONTEXTS_HANDLE => {
                            self.available_audio_contexts.into_char_value()
                        }
                        x if x == SUPPORTED_AUDIO_CONTEXTS_HANDLE => {
                            self.supported_audio_contexts.into_char_value()
                        }
                        x if self.is_source_locations_handle(x) => {
                            self.source_audio_locations.as_ref().unwrap().into_char_value()
                        }
                        x if self.is_sink_locations_handle(x) => {
                            self.sink_audio_locations.as_ref().unwrap().into_char_value()
                        }
                        pac_handle => {
                            let Some(ref pac) = self.published_audio_capabilities.get(&pac_handle)
                            else {
                                responder.error(GattError::InvalidHandle);
                                continue;
                            };
                            pac.encode()
                        }
                    };
                    responder.respond(&value[offset..]);
                    continue;
                }
                // TODO(b/335293412): support optional writes.
                Write { responder, .. } => {
                    responder.error(GattError::WriteNotPermitted);
                    continue;
                }
                // We don't do anything complex with ClientConfiguration for now.
                ClientConfiguration { .. } => continue,
                _ => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_common::core::{CodecId, CodingFormat};
    use bt_common::generic_audio::codec_capabilities::*;
    use bt_common::generic_audio::AudioLocation;
    use bt_common::PeerId;
    use bt_gatt::server;
    use bt_gatt::server::NotificationType;
    use bt_gatt::test_utils::{FakeServer, FakeServerEvent, FakeTypes};
    use bt_gatt::types::ServiceKind;

    use futures::{FutureExt, StreamExt};
    use std::collections::HashSet;

    use crate::AvailableContexts;

    // Builder for a server with:
    // - 1 sink and 1 source PAC characteristics
    // - sink audio locations
    fn default_server_builder() -> ServerBuilder {
        ServerBuilder::new()
            .add_sink(vec![PacRecord {
                codec_id: CodecId::Assigned(CodingFormat::ALawLog),
                codec_specific_capabilities: vec![CodecCapability::SupportedFrameDurations(
                    FrameDurationSupport::BothNoPreference,
                )],
                metadata: vec![],
            }])
            .set_sink_locations(AudioLocations {
                locations: HashSet::from([AudioLocation::FrontLeft, AudioLocation::FrontRight]),
            })
            .add_source(vec![
                PacRecord {
                    codec_id: CodecId::Assigned(CodingFormat::ALawLog),
                    codec_specific_capabilities: vec![CodecCapability::SupportedFrameDurations(
                        FrameDurationSupport::BothNoPreference,
                    )],
                    metadata: vec![],
                },
                PacRecord {
                    codec_id: CodecId::Assigned(CodingFormat::MuLawLog),
                    codec_specific_capabilities: vec![CodecCapability::SupportedFrameDurations(
                        FrameDurationSupport::BothNoPreference,
                    )],
                    metadata: vec![],
                },
            ])
            .add_source(vec![])
    }

    #[test]
    fn build_server() {
        let server = default_server_builder()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Conversational, ContextType::Media]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::from([ContextType::Media]), HashSet::new()),
            )
            .expect("should succeed");
        assert_eq!(server.published_audio_capabilities.len(), 2);

        assert_eq!(server.supported_audio_contexts.handle.0, 1);
        assert_eq!(
            server.supported_audio_contexts.sink,
            HashSet::from([ContextType::Conversational, ContextType::Media])
        );
        assert_eq!(server.supported_audio_contexts.source, HashSet::from([ContextType::Media]));

        assert_eq!(server.available_audio_contexts.handle.0, 2);
        assert_eq!(
            server.available_audio_contexts.sink,
            AvailableContexts::Available(HashSet::from([ContextType::Media]))
        );
        assert_eq!(server.available_audio_contexts.source, AvailableContexts::NotAvailable);

        // Should have 1 sink PAC characteristic with audio locations.
        let location_char = server.sink_audio_locations.as_ref().expect("should exist");
        assert_eq!(location_char.handle.0, 3);
        assert_eq!(
            location_char.locations.locations,
            HashSet::from([AudioLocation::FrontLeft, AudioLocation::FrontRight])
        );

        let mut sink_iter =
            server.published_audio_capabilities.iter().filter(|(_handle, pac)| pac.is_sink());
        let sink_char = sink_iter.next().expect("should exist");
        assert_eq!(sink_char.0, &Handle(4));
        assert_eq!(sink_char.1.pac_records().len(), 1);
        assert!(sink_iter.next().is_none());

        // Should have 1 source PAC characteristic w/o audio locations.
        assert!(server.source_audio_locations.is_none());
        let mut source_iter =
            server.published_audio_capabilities.iter().filter(|(_handle, pac)| pac.is_source());
        let source_char = source_iter.next().expect("should exist");
        assert_eq!(source_char.0, &Handle(5));
        assert_eq!(source_char.1.pac_records().len(), 2);
        assert_eq!(source_iter.next(), None);
    }

    #[test]
    fn build_server_error() {
        // No sink or source PACs.
        assert!(ServerBuilder::new()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Conversational, ContextType::Media]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::from([ContextType::Media]), HashSet::new()),
            )
            .is_err());

        // Sink audio context in available not in supported.
        assert!(default_server_builder()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Conversational, ContextType::Media]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::from([ContextType::Alerts]), HashSet::new()),
            )
            .is_err());

        // Sink audio context in available not in supported.
        assert!(default_server_builder()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Conversational, ContextType::Media]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::from([]), HashSet::from([ContextType::EmergencyAlarm])),
            )
            .is_err());
    }

    #[test]
    fn publish_server() {
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

        let mut server = default_server_builder()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Media]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::new(), HashSet::new()),
            )
            .unwrap();

        // Server should be pending still since GATT server not establihsed.
        let Poll::Pending = server.next().poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };

        let (fake_gatt_server, mut event_receiver) = FakeServer::new();

        // Event stream should be pending still since service not published.
        let mut event_stream = event_receiver.next();
        let Poll::Pending = event_stream.poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };

        let _ = server.publish(fake_gatt_server).expect("should succeed");

        // Server should poll on local server state.
        let Poll::Pending = server.next().poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };

        // Should receive event that GATT service was published.
        let Poll::Ready(Some(FakeServerEvent::Published { id, definition })) =
            event_stream.poll_unpin(&mut noop_cx)
        else {
            panic!("Should be published");
        };
        assert_eq!(id, server::ServiceId::new(1));
        assert_eq!(definition.characteristics().collect::<Vec<_>>().len(), 5);
        assert_eq!(definition.kind(), ServiceKind::Primary);
        assert_eq!(definition.uuid(), crate::PACS_UUID);

        // Server can only be published once.
        let (fake_gatt_server, _) = FakeServer::new();
        assert!(server.publish(fake_gatt_server).is_err());
    }

    #[test]
    fn read_from_server() {
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

        let mut server = default_server_builder()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Media]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::from([ContextType::Media]), HashSet::new()),
            )
            .unwrap();

        let (fake_gatt_server, mut event_receiver) = FakeServer::new();
        let _ = server.publish(fake_gatt_server.clone()).expect("should succeed");

        // Server should poll on local server state.
        let Poll::Pending = server.next().poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };

        // Should receive event that GATT service was published.
        let mut event_stream = event_receiver.next();
        let Poll::Ready(Some(FakeServerEvent::Published { id, .. })) =
            event_stream.poll_unpin(&mut noop_cx)
        else {
            panic!("Should be published");
        };

        // Fake an incoming read from a remote peer.
        let available_char_handle = server.available_audio_contexts.handle;
        fake_gatt_server.incoming_read(PeerId(0x01), id, available_char_handle, 0);

        // Server should still be pending.
        let Poll::Pending = server.next().poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };

        // We should received read response.
        let Poll::Ready(Some(FakeServerEvent::ReadResponded { handle, value, .. })) =
            event_stream.poll_unpin(&mut noop_cx)
        else {
            panic!("Should be published");
        };
        assert_eq!(handle, available_char_handle);
        assert_eq!(value.expect("should be ok"), vec![0x04, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn update_available() {
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());

        let mut server = default_server_builder()
            .build::<FakeTypes>(
                AudioContexts::new(
                    HashSet::from([ContextType::Media, ContextType::Alerts]),
                    HashSet::from([ContextType::Media]),
                ),
                AudioContexts::new(HashSet::from([ContextType::Media]), HashSet::new()),
            )
            .unwrap();

        let (fake_gatt_server, mut event_receiver) = FakeServer::new();
        let _ = server.publish(fake_gatt_server.clone()).expect("should succeed");

        // Server should poll on local server state.
        let Poll::Pending = server.next().poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };

        // Should receive event that GATT service was published.
        let mut event_stream = event_receiver.next();
        let Poll::Ready(Some(FakeServerEvent::Published { id, .. })) =
            event_stream.poll_unpin(&mut noop_cx)
        else {
            panic!("Should be published");
        };

        // Mimic a remote peer de-registering for notifications.
        fake_gatt_server.incoming_client_configuration(
            PeerId(1),
            id,
            AVAILABLE_AUDIO_CONTEXTS_HANDLE,
            NotificationType::Disable,
        );

        // Update available context information.
        let updated = server.update_available(AudioContexts {
            sink: HashSet::from([ContextType::Alerts]),
            source: HashSet::from([ContextType::Media, ContextType::Conversational]),
        });
        assert_eq!(
            updated.sink,
            AvailableContexts::Available(HashSet::from([ContextType::Alerts]))
        );
        assert_eq!(
            updated.source,
            AvailableContexts::Available(HashSet::from([ContextType::Media]))
        );
        // No event received since we haven't registered for notifications.
        let Poll::Pending = event_stream.poll_unpin(&mut noop_cx) else {
            panic!("No peer registered for notifications");
        };

        // Mimic a remote peer registering for notifications.
        fake_gatt_server.incoming_client_configuration(
            PeerId(1),
            id,
            AVAILABLE_AUDIO_CONTEXTS_HANDLE,
            NotificationType::Notify,
        );
        // Poll on server to process client config request.
        let Poll::Pending = server.next().poll_unpin(&mut noop_cx) else {
            panic!("Should be pending");
        };
        // Update available context information.
        let updated = server.update_available(AudioContexts {
            sink: HashSet::from([ContextType::Media, ContextType::Alerts]),
            source: HashSet::from([ContextType::Media]),
        });
        assert_eq!(
            updated.sink,
            AvailableContexts::Available(HashSet::from([ContextType::Media, ContextType::Alerts]))
        );
        assert_eq!(
            updated.source,
            AvailableContexts::Available(HashSet::from([ContextType::Media]))
        );
        // Should have notified the updated value.
        let Poll::Ready(Some(FakeServerEvent::Notified { service_id, handle, value, peers })) =
            event_stream.poll_unpin(&mut noop_cx)
        else {
            panic!("Should be notified");
        };
        assert_eq!(service_id, id);
        assert_eq!(handle, AVAILABLE_AUDIO_CONTEXTS_HANDLE);
        assert_eq!(value, vec![0x04, 0x04, 0x04, 0x00]);
        assert_eq!(peers, vec![PeerId(1)]);

        // Update available with the same context information.
        let _ = server.update_available(AudioContexts {
            sink: HashSet::from([ContextType::Media, ContextType::Alerts]),
            source: HashSet::from([ContextType::Media, ContextType::Conversational]),
        });
        // No event received since available context hasn't changed.
        let Poll::Pending = event_stream.poll_unpin(&mut noop_cx) else {
            panic!("Should not be notified");
        };
    }
}
