// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::error::EventError;
use crate::identity::ComponentIdentity;
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fidl_table_validation::ValidFidlTable;
use moniker::ExtendedMoniker;
use std::sync::Arc;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_inspect as finspect,
    fidl_fuchsia_logger as flogger,
};

/// Event types that contain singleton data. When these events are cloned, their singleton data
/// won't be cloned.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EventType {
    LogSinkRequested,
    InspectSinkRequested,
}

impl AsRef<str> for EventType {
    fn as_ref(&self) -> &str {
        match &self {
            Self::LogSinkRequested => "log_sink_requested",
            Self::InspectSinkRequested => "inspect_sink_requested",
        }
    }
}

/// An event that is emitted and consumed.
#[derive(Debug)]
pub struct Event {
    /// The contents of the event.
    pub payload: EventPayload,
    pub timestamp: zx::BootInstant,
}

impl Event {
    pub fn ty(&self) -> EventType {
        match &self.payload {
            EventPayload::LogSinkRequested(_) => EventType::LogSinkRequested,
            EventPayload::InspectSinkRequested(_) => EventType::InspectSinkRequested,
        }
    }
}

/// The contents of the event depending on the type of event.
#[derive(Debug)]
pub enum EventPayload {
    LogSinkRequested(LogSinkRequestedPayload),
    InspectSinkRequested(InspectSinkRequestedPayload),
}

pub struct InspectSinkRequestedPayload {
    /// The component that is connecting to `InspectSink`.
    pub component: Arc<ComponentIdentity>,
    /// The stream containing requests made on the `InspectSink` channel by the component.
    pub request_stream: finspect::InspectSinkRequestStream,
}

impl std::fmt::Debug for InspectSinkRequestedPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectSinkRequestedPayload").field("component", &self.component).finish()
    }
}

/// Payload for a connection to the `LogSink` protocol.
pub struct LogSinkRequestedPayload {
    /// The component that is connecting to `LogSink`.
    pub component: Arc<ComponentIdentity>,
    /// The stream containing requests made on the `LogSink` channel by the component.
    pub request_stream: flogger::LogSinkRequestStream,
}

impl std::fmt::Debug for LogSinkRequestedPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogSinkRequestedPayload").field("component", &self.component).finish()
    }
}

#[derive(Debug, ValidFidlTable)]
#[fidl_table_src(fcomponent::EventHeader)]
#[fidl_table_strict]
pub struct ValidatedEventHeader {
    event_type: fcomponent::EventType,
    component_url: String,
    moniker: String,
    timestamp: zx::BootInstant,
}

#[derive(Debug, ValidFidlTable)]
#[fidl_table_src(fcomponent::Event)]
#[fidl_table_strict]
pub struct ValidatedEvent {
    /// Information about the component for which this event was generated.
    pub header: ValidatedEventHeader,

    pub payload: fcomponent::EventPayload,
}

impl TryFrom<fcomponent::Event> for Event {
    type Error = EventError;

    fn try_from(event: fcomponent::Event) -> Result<Event, Self::Error> {
        if let Ok(event) = ValidatedEvent::try_from(event) {
            let identity = ComponentIdentity::new(
                ExtendedMoniker::parse_str(&event.header.moniker)?,
                &event.header.component_url,
            );

            match event.header.event_type {
                fcomponent::EventType::CapabilityRequested => match event.payload {
                    fcomponent::EventPayload::CapabilityRequested(capability_requested) => {
                        let name =
                            capability_requested.name.ok_or(EventError::MissingField("name"))?;

                        if name == flogger::LogSinkMarker::PROTOCOL_NAME {
                            let capability = capability_requested
                                .capability
                                .ok_or(EventError::MissingField("capability"))?;
                            let request_stream =
                                ServerEnd::<flogger::LogSinkMarker>::new(capability).into_stream();
                            Ok(Event {
                                timestamp: event.header.timestamp,
                                payload: EventPayload::LogSinkRequested(LogSinkRequestedPayload {
                                    component: Arc::new(identity),
                                    request_stream,
                                }),
                            })
                        } else if name == finspect::InspectSinkMarker::PROTOCOL_NAME {
                            let capability = capability_requested
                                .capability
                                .ok_or(EventError::MissingField("capability"))?;
                            let request_stream =
                                ServerEnd::<finspect::InspectSinkMarker>::new(capability)
                                    .into_stream();
                            Ok(Event {
                                timestamp: event.header.timestamp,
                                payload: EventPayload::InspectSinkRequested(
                                    InspectSinkRequestedPayload {
                                        component: Arc::new(identity),
                                        request_stream,
                                    },
                                ),
                            })
                        } else {
                            Err(EventError::IncorrectName {
                                received: name,
                                expected: flogger::LogSinkMarker::PROTOCOL_NAME,
                            })
                        }
                    }
                    _ => Err(EventError::UnknownResult(event.payload)),
                },
                _ => Err(EventError::InvalidEventType(event.header.event_type)),
            }
        } else {
            Err(EventError::MissingField("Payload or header is missing"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::testing::create_log_sink_requested_event;
    use assert_matches::assert_matches;
    use fidl_fuchsia_logger::LogSinkMarker;
    use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_inspect as finspect};

    fn create_inspect_sink_requested_event(
        target_moniker: String,
        target_url: String,
        capability: zx::Channel,
    ) -> fcomponent::Event {
        fcomponent::Event {
            header: Some(fcomponent::EventHeader {
                event_type: Some(fcomponent::EventType::CapabilityRequested),
                moniker: Some(target_moniker),
                component_url: Some(target_url),
                timestamp: Some(zx::BootInstant::get()),
                ..Default::default()
            }),
            payload: Some(fcomponent::EventPayload::CapabilityRequested(
                fcomponent::CapabilityRequestedPayload {
                    name: Some(finspect::InspectSinkMarker::PROTOCOL_NAME.into()),
                    capability: Some(capability),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
    }

    #[fuchsia::test]
    fn inspect_sink_capability_requested() {
        let _exec = fuchsia_async::LocalExecutor::default();
        let (_, inspect_sink_server_end) =
            fidl::endpoints::create_endpoints::<finspect::InspectSinkMarker>();
        let event = create_inspect_sink_requested_event(
            "a/b".into(),
            "".into(),
            inspect_sink_server_end.into_channel(),
        );
        let actual = event.try_into().unwrap();

        assert_matches!(actual, Event { payload, .. } => {
            assert_matches!(payload,
                EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                    component, ..
                }) => {
                    assert_eq!(component, ComponentIdentity::from(vec!["a", "b"]).into());
                })
        });
    }

    #[fuchsia::test]
    fn log_sink_capability_requested() {
        let _exec = fuchsia_async::LocalExecutor::default();
        let (_, log_sink_server_end) = fidl::endpoints::create_endpoints::<LogSinkMarker>();
        let event = create_log_sink_requested_event(
            "a/b".into(),
            "".into(),
            log_sink_server_end.into_channel(),
        );
        let actual = event.try_into().unwrap();

        assert_matches!(actual, Event { payload, .. } => {
            assert_matches!(payload,
                EventPayload::LogSinkRequested(LogSinkRequestedPayload {
                    component, ..
                }) => {
                    assert_eq!(component, ComponentIdentity::from(vec!["a", "b"]).into());
                })
        });
    }
}
