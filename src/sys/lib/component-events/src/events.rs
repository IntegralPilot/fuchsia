// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl::endpoints::{ProtocolMarker, ServerEnd};
use fuchsia_component::client::connect_to_protocol_at_path;
use futures::task::{Context, Poll};
use futures::{TryFuture, ready};
use pin_project_lite::pin_project;
use std::collections::VecDeque;
use std::sync::LazyLock;
use thiserror::Error;
use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_io as fio};

/// The path of the static event stream that, by convention, synchronously listens for
/// Resolved events.
pub static START_COMPONENT_TREE_STREAM: LazyLock<String> =
    LazyLock::new(|| "StartComponentTree".into());

/// Returns the string name for the given `event_type`
pub fn event_name(event_type: &fcomponent::EventType) -> String {
    match event_type {
        fcomponent::EventType::CapabilityRequested => "capability_requested",
        fcomponent::EventType::Discovered => unreachable!("This isn't used anymore"),
        fcomponent::EventType::Destroyed => "destroyed",
        fcomponent::EventType::Resolved => "resolved",
        fcomponent::EventType::Unresolved => "unresolved",
        fcomponent::EventType::Started => "started",
        fcomponent::EventType::Stopped => "stopped",
        fcomponent::EventType::DebugStarted => "debug_started",
    }
    .to_string()
}

pin_project! {
    pub struct EventStream {
        stream: fcomponent::EventStreamProxy,
        buffer: VecDeque<fcomponent::Event>,
        #[pin]
        fut: Option<<fcomponent::EventStreamProxy as fcomponent::EventStreamProxyInterface>::GetNextResponseFut>,
    }
}

#[derive(Debug, Error, Clone)]
pub enum EventStreamError {
    #[error("Stream terminated unexpectedly")]
    StreamClosed,
}

impl EventStream {
    pub fn new(stream: fcomponent::EventStreamProxy) -> Self {
        Self { stream, buffer: VecDeque::new(), fut: None }
    }

    pub fn open_at_path_pipelined(path: impl Into<String>) -> Result<Self, Error> {
        Ok(Self::new(connect_to_protocol_at_path::<fcomponent::EventStreamMarker>(&path.into())?))
    }

    pub async fn open_at_path(path: impl Into<String>) -> Result<Self, Error> {
        let event_stream =
            connect_to_protocol_at_path::<fcomponent::EventStreamMarker>(&path.into())?;
        event_stream.wait_for_ready().await?;
        Ok(Self::new(event_stream))
    }

    pub async fn open() -> Result<Self, Error> {
        let event_stream = connect_to_protocol_at_path::<fcomponent::EventStreamMarker>(
            "/svc/fuchsia.component.EventStream",
        )?;
        event_stream.wait_for_ready().await?;
        Ok(Self::new(event_stream))
    }

    pub fn open_pipelined() -> Result<Self, Error> {
        Ok(Self::new(connect_to_protocol_at_path::<fcomponent::EventStreamMarker>(
            "/svc/fuchsia.component.EventStream",
        )?))
    }
}

impl futures::Stream for EventStream {
    type Item = fcomponent::Event;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // Return queued up events when possible.
        if let Some(event) = this.buffer.pop_front() {
            return Poll::Ready(Some(event));
        }

        // Otherwise, listen for more events.
        if let None = this.fut.as_mut().as_pin_mut() {
            this.fut.set(Some(this.stream.get_next()));
        }

        let step = ready!(this.fut.as_mut().as_pin_mut().unwrap().try_poll(cx));
        this.fut.set(None);

        match step {
            Ok(events) => {
                let mut iter = events.into_iter();
                let ret = iter.next().unwrap();
                // Store leftover events for subsequent polls.
                while let Some(leftover) = iter.next() {
                    this.buffer.push_back(leftover);
                }
                Poll::Ready(Some(ret))
            }
            Err(_) => Poll::Ready(None),
        }
    }
}

/// Common features of any event - event type, target moniker, conversion function
pub trait Event: TryFrom<fcomponent::Event, Error = anyhow::Error> {
    const TYPE: fcomponent::EventType;
    const NAME: &'static str;

    fn target_moniker(&self) -> &str;
    fn component_url(&self) -> &str;
    fn timestamp(&self) -> zx::BootInstant;
    fn is_ok(&self) -> bool;
    fn is_err(&self) -> bool;
}

#[derive(Copy, Debug, PartialEq, Eq, Clone, Ord, PartialOrd)]
/// Simplifies the exit status represented by an Event. All stop status values
/// that indicate failure are crushed into `Crash`.
pub enum ExitStatus {
    Clean,
    Crash(i32),
}

impl From<i32> for ExitStatus {
    fn from(exit_status: i32) -> Self {
        match exit_status {
            0 => ExitStatus::Clean,
            _ => ExitStatus::Crash(exit_status),
        }
    }
}

#[derive(Debug)]
struct EventHeader {
    event_type: fcomponent::EventType,
    component_url: String,
    moniker: String,
    timestamp: zx::BootInstant,
}

impl TryFrom<fcomponent::EventHeader> for EventHeader {
    type Error = anyhow::Error;

    fn try_from(header: fcomponent::EventHeader) -> Result<Self, Self::Error> {
        let event_type = header.event_type.ok_or_else(|| format_err!("No event type"))?;
        let component_url = header.component_url.ok_or_else(|| format_err!("No component url"))?;
        let moniker = header.moniker.ok_or_else(|| format_err!("No moniker"))?;
        let timestamp = header
            .timestamp
            .ok_or_else(|| format_err!("Missing timestamp from the Event object"))?;
        Ok(EventHeader { event_type, component_url, moniker, timestamp })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct EventError {
    pub description: String,
}

/// The macro defined below will automatically create event classes corresponding
/// to their events.fidl and hooks.rs counterparts. Every event class implements
/// the Event and Handler traits. These minimum requirements allow every event to
/// be handled by the events client library.

/// Creates an event class based on event type and an optional payload
/// * event_type -> FIDL name for event type
/// * payload -> If an event has a payload, describe the additional params:
///   * name -> FIDL name for the payload
///   * data -> If a payload contains data items, describe the additional params:
///     * name -> FIDL name for the data item
///     * ty -> Rust type for the data item
///   * client_protocols -> If a payload contains client-side protocols, describe
///     the additional params:
///     * name -> FIDL name for the protocol
///     * ty -> Rust type for the protocol proxy
///   * server_protocols -> If a payload contains server-side protocols, describe
///     the additional params:
///     * name -> FIDL name for the protocol
// TODO(https://fxbug.dev/42131403): This marco is getting complicated. Consider replacing it
//                  with a procedural macro.
macro_rules! create_event {
    // Entry points
    (
        event_type: $event_type:ident,
        event_name: $event_name:ident,
        payload: {
            data: {$(
                {
                    name: $data_name:ident,
                    ty: $data_ty:ty,
                }
            )*},
            client_protocols: {$(
                {
                    name: $client_protocol_name:ident,
                    ty: $client_protocol_ty:ty,
                }
            )*},
            server_protocols: {$(
                {
                    name: $server_protocol_name:ident,
                }
            )*},
        },
        error_payload: {
            $(
                {
                    name: $error_data_name:ident,
                    ty: $error_data_ty:ty,
                }
            )*
        }
    ) => {
        paste::paste! {
            #[derive(Debug)]
            pub struct [<$event_type Payload>] {
                $(pub $client_protocol_name: $client_protocol_ty,)*
                $(pub $server_protocol_name: Option<zx::Channel>,)*
                $(pub $data_name: $data_ty,)*
            }

            #[derive(Debug)]
            pub struct [<$event_type Error>] {
                $(pub $error_data_name: $error_data_ty,)*
                pub description: String,
            }

            #[derive(Debug)]
            pub struct $event_type {
                header: EventHeader,
                result: Result<[<$event_type Payload>], [<$event_type Error>]>,
            }

            impl $event_type {
                pub fn result<'a>(&'a self) -> Result<&'a [<$event_type Payload>], &'a [<$event_type Error>]> {
                    self.result.as_ref()
                }

                $(
                    pub fn [<take_ $server_protocol_name>]<T: ProtocolMarker>(&mut self)
                            -> Option<T::RequestStream> {
                        self.result.as_mut()
                            .ok()
                            .and_then(|payload| payload.$server_protocol_name.take())
                            .map(|channel| {
                                let server_end = ServerEnd::<T>::new(channel);
                                server_end.into_stream()
                            })
                    }
                )*
            }

            impl Event for $event_type {
                const TYPE: fcomponent::EventType = fcomponent::EventType::$event_type;
                const NAME: &'static str = stringify!($event_name);

                fn target_moniker(&self) -> &str {
                    &self.header.moniker
                }

                fn component_url(&self) -> &str {
                    &self.header.component_url
                }

                fn timestamp(&self) -> zx::BootInstant {
                    self.header.timestamp
                }

                fn is_ok(&self) -> bool {
                    self.result.is_ok()
                }

                fn is_err(&self) -> bool {
                    self.result.is_err()
                }
            }

            impl TryFrom<fcomponent::Event> for $event_type {
                type Error = anyhow::Error;

                fn try_from(event: fcomponent::Event) -> Result<Self, Self::Error> {
                    // Extract the payload from the Event object.
                    let result = match event.payload {
                        Some(payload) => {
                            // This payload will be unused for event types that have no additional
                            // fields.
                            #[allow(unused)]
                            let payload = match payload {
                                fcomponent::EventPayload::$event_type(payload) => Ok(payload),
                                _ => Err(format_err!("Incorrect payload type, {:?}", payload)),
                            }?;

                            // Extract the additional data from the Payload object.
                            $(
                                let $data_name: $data_ty = payload.$data_name.coerce().ok_or(
                                    format_err!("Missing {} from {} object",
                                        stringify!($data_name), stringify!($event_type))
                                )?;
                            )*

                            // Extract the additional protocols from the Payload object.
                            $(
                                let $client_protocol_name: $client_protocol_ty = payload.$client_protocol_name.ok_or(
                                    format_err!("Missing {} from {} object",
                                        stringify!($client_protocol_name), stringify!($event_type))
                                )?.into_proxy();
                            )*
                            $(
                                let $server_protocol_name: Option<zx::Channel> =
                                    Some(payload.$server_protocol_name.ok_or(
                                        format_err!("Missing {} from {} object",
                                            stringify!($server_protocol_name), stringify!($event_type))
                                    )?);
                            )*

                            #[allow(dead_code)]
                            let payload = paste::paste! {
                                [<$event_type Payload>] {
                                    $($data_name,)*
                                    $($client_protocol_name,)*
                                    $($server_protocol_name,)*
                                }
                            };

                            Ok(Ok(payload))
                        },
                        None => Err(format_err!("Missing event_result from Event object")),
                    }?;

                    let event = {
                        let header = event.header
                            .ok_or(format_err!("Missing Event header"))
                            .and_then(|header| EventHeader::try_from(header))?;

                        if header.event_type != Self::TYPE {
                            return Err(format_err!("Incorrect event type"));
                        }

                        $event_type { header, result }
                    };
                    Ok(event)
                }
            }
        }
    };
    ($event_type:ident, $event_name:ident) => {
        create_event!(event_type: $event_type, event_name: $event_name,
                      payload: {
                          data: {},
                          client_protocols: {},
                          server_protocols: {},
                      },
                      error_payload: {});
    };
}

// To create a class for an event, use the above macro here.
create_event!(Destroyed, destroyed);
create_event!(Resolved, resolved);
create_event!(Unresolved, unresolved);
create_event!(Started, started);
create_event!(
    event_type: Stopped,
    event_name: stopped,
    payload: {
        data: {
            {
                name: status,
                ty: ExitStatus,
            }
            {
                name: exit_code,
                ty: Option<i64>,
            }
        },
        client_protocols: {},
        server_protocols: {},
    },
    error_payload: {}
);
create_event!(
    event_type: CapabilityRequested,
    event_name: capability_requested,
    payload: {
        data: {
            {
                name: name,
                ty: String,
            }
        },
        client_protocols: {},
        server_protocols: {
            {
                name: capability,
            }
        },
    },
    error_payload: {
        {
            name: name,
            ty: String,
        }
    }
);
create_event!(
    event_type: DebugStarted,
    event_name: debug_started,
    payload: {
        data: {
            {
                name: break_on_start,
                ty: zx::EventPair,
            }
        },
        client_protocols: {
            {
                name: runtime_dir,
                ty: fio::DirectoryProxy,
            }
        },
        server_protocols: {},
    },
    error_payload: {}
);

trait Coerce<T> {
    fn coerce(self) -> Option<T>;
}

impl<T> Coerce<T> for Option<T> {
    fn coerce(self) -> Option<T> {
        self
    }
}

impl<T> Coerce<Option<T>> for Option<T> {
    fn coerce(self) -> Option<Option<T>> {
        Some(self)
    }
}

impl Coerce<ExitStatus> for Option<i32> {
    fn coerce(self) -> Option<ExitStatus> {
        self.map(Into::into)
    }
}
