// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A module for managing message passing between Netlink and its clients.

use futures::Stream;
use netlink_packet_core::{
    NetlinkBuffer, NetlinkDeserializable, NetlinkHeader, NetlinkMessage, NetlinkPayload,
    NetlinkSerializable,
};
use netlink_packet_route::RouteNetlinkMessageParseError;
use netlink_packet_utils::nla::NlaError;
use netlink_packet_utils::{DecodeError, Parseable, ParseableParametrized};
use std::fmt::Debug;

use crate::multicast_groups::ModernGroup;
use crate::netlink_packet;
use crate::netlink_packet::errno::Errno;

/// A type capable of sending messages, `M`, from Netlink to a client.
pub trait Sender<M>: Clone + Send + Sync {
    /// Sends the given message to the client.
    ///
    /// If the message is a multicast, `group` will hold a `Some`; `None` for
    /// unicast messages.
    ///
    /// Implementors must ensure this call does not block.
    fn send(&mut self, message: NetlinkMessage<M>, group: Option<ModernGroup>);
}

/// A type capable of receiving messages, `M`, from a client to Netlink.
///
/// [`Stream`] already provides a sufficient interface for this purpose.
pub trait Receiver<M, C>:
    Stream<Item: UnvalidatedNetlinkMessage<Message = M, Credentials = C>> + Send
where
    M: Send + MessageWithPermission,
    C: Send,
{
}

/// Blanket implementation allows any [`Stream`] to be used as a [`Receiver`].
impl<M, C, S> Receiver<M, C> for S
where
    M: Send + MessageWithPermission,
    C: Send,
    S: Stream<Item: UnvalidatedNetlinkMessage<Message = M, Credentials = C>> + Send,
{
}

/// A permission that is required from the sender when processing a netlink
/// request.
pub enum Permission {
    /// GET messages in NETLINK_ROUTE.
    NetlinkRouteRead,

    /// SET/NEW/DELETE messages in NETLINK_ROUTE.
    NetlinkRouteWrite,

    /// Request and read diagnostic information.
    NetlinkSockDiagRead,

    /// Disconnect sockets with SOCK_DESTROY.
    NetlinkSockDiagDestroy,
}

/// An object responsible to validating permissions for netlink clients.
pub trait AccessControl<C>: Clone {
    /// Returns true if a client with the specified credentials has the
    /// specified permission.
    fn grant_assess(&self, creds: &C, permission: Permission) -> Result<(), Errno>;
}

/// A message that may require special permissions from the sender to be
/// processed.
pub trait MessageWithPermission {
    /// Returns the permission that's required in order to process this message.
    fn permission(&self) -> Permission;
}

/// An error observed when parsing a netlink message.
#[derive(Debug)]
pub struct ParseError {
    /// The error encountered during parsing.
    pub error: DecodeError,
    /// The header on the original message.
    ///
    /// If a header was not able to be extracted from the original message,
    /// `None` is set.
    pub header: Option<NetlinkHeader>,
}

/// A trait abstracting netlink messages that may already be parsed or still
/// need to go through parsing.
pub trait MaybeParsedNetlinkMessage {
    /// The inner message type potentially contained by this message.
    type Message: MessageWithPermission;

    /// Parses the message, returning an owned [`NetlinkMessage`] on success.
    fn try_into_parsed(self) -> Result<NetlinkMessage<Self::Message>, ParseError>;
}

impl<M: MessageWithPermission> MaybeParsedNetlinkMessage for NetlinkMessage<M> {
    type Message = M;
    fn try_into_parsed(self) -> Result<NetlinkMessage<M>, ParseError> {
        Ok(self)
    }
}

/// An unparsed netlink message backed by the bytes in `B`.
pub struct UnparsedNetlinkMessage<B, M: NetlinkDeserializable> {
    data: B,
    options: M::DeserializeOptions,
    _marker: std::marker::PhantomData<M>,
}

impl<B, M: NetlinkDeserializable> UnparsedNetlinkMessage<B, M> {
    /// Creates a new `UnparsedNetlinkMessage`.
    pub fn new(data: B, options: M::DeserializeOptions) -> Self {
        Self { data, options, _marker: std::marker::PhantomData }
    }
}

impl<M, B> MaybeParsedNetlinkMessage for UnparsedNetlinkMessage<B, M>
where
    B: AsRef<[u8]>,
    M: NetlinkDeserializable + MessageWithPermission,
    M::Error: Into<DecodeError>,
{
    type Message = M;

    fn try_into_parsed(self) -> Result<NetlinkMessage<M>, ParseError> {
        let Self { data, options, _marker } = self;
        let data = data.as_ref();
        let netlink_buffer =
            NetlinkBuffer::new(&data).map_err(|error| ParseError { error, header: None })?;
        NetlinkMessage::<M>::parse_with_param(&netlink_buffer, options).map_err(|error| {
            ParseError {
                error,
                // Silently drop the parsing error here, the error from parsing the
                // NetlinkMessage itself should be enough.
                header: NetlinkHeader::parse(&netlink_buffer).ok(),
            }
        })
    }
}

/// The outcome of validating a netlink message.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum ValidationError {
    /// The message failed to parse.
    Parse(ParseError),
    /// The provided credentials are insufficient for the requested operation.
    Permission { header: NetlinkHeader, error: Errno },
}

// TODO(https://fxbug.dev/450959280): Move this close to the netlink crates, so
// we have more control of the error types more locally,
fn nla_error_to_errno(error: &NlaError) -> Errno {
    match error {
        NlaError::BufferTooSmall { .. }
        | NlaError::LengthMismatch { .. }
        | NlaError::InvalidLength { .. } => Errno::EINVAL,
    }
}

// TODO(https://fxbug.dev/450959280): Move this close to the netlink crates, so
// we have more control of the error types more locally,
fn route_netlink_error_to_errno(error: &RouteNetlinkMessageParseError) -> Errno {
    match error {
        RouteNetlinkMessageParseError::ParseBuffer(decode_error)
        | RouteNetlinkMessageParseError::InvalidLinkMessage(decode_error) => {
            decode_error_to_errno(decode_error)
        }
        RouteNetlinkMessageParseError::InvalidRouteMessage(_)
        | RouteNetlinkMessageParseError::InvalidAddrMessage(_)
        | RouteNetlinkMessageParseError::InvalidPrefixMessage(_)
        | RouteNetlinkMessageParseError::InvalidFibRuleMessage(_)
        | RouteNetlinkMessageParseError::InvalidTcMessage(_)
        | RouteNetlinkMessageParseError::InvalidNsidMessage(_)
        | RouteNetlinkMessageParseError::InvalidNeighbourMessage(_)
        | RouteNetlinkMessageParseError::InvalidNeighbourTableMessage(_)
        | RouteNetlinkMessageParseError::InvalidNeighbourDiscoveryUserOptionMessage(_) => {
            Errno::EINVAL
        }
        RouteNetlinkMessageParseError::UnknownMessageType(_) => Errno::ENOTSUP,
    }
}

// TODO(https://fxbug.dev/450959280): Move this close to the netlink crates, so
// we have more control of the error types more locally,
fn decode_error_to_errno(error: &DecodeError) -> Errno {
    match error {
        DecodeError::InvalidMACAddress
        | DecodeError::InvalidIPAddress
        | DecodeError::Utf8Error(_)
        | DecodeError::InvalidU8
        | DecodeError::InvalidU16
        | DecodeError::InvalidU32
        | DecodeError::InvalidU64
        | DecodeError::InvalidU128
        | DecodeError::InvalidI32
        | DecodeError::InvalidBufferLength { .. } => Errno::EINVAL,
        DecodeError::Nla(nla_error) => nla_error_to_errno(nla_error),
        DecodeError::Other(error) => {
            if let Some(error) = error.downcast_ref::<RouteNetlinkMessageParseError>() {
                return route_netlink_error_to_errno(error);
            }
            if let Some(error) = error.downcast_ref::<netlink_packet_utils::DecodeError>() {
                return decode_error_to_errno(error);
            }
            Errno::EINVAL
        }
        DecodeError::FailedToParseNlMsgError(error)
        | DecodeError::FailedToParseNlMsgDone(error)
        | DecodeError::FailedToParseMessageWithType { message_type: _, source: error }
        | DecodeError::FailedToParseNetlinkHeader(error) => decode_error_to_errno(error),
    }
}

impl ValidationError {
    /// Creates an error message response from this error, if one can be
    /// created.
    pub fn into_error_message<M: NetlinkSerializable>(self) -> Option<NetlinkMessage<M>> {
        match self {
            ValidationError::Parse(ParseError { error, header }) => {
                // If we couldn't parse at least a header, we can't respond.
                let header = header?;
                // NB: Decode error from netlink_core doesn't quite have enough
                // granularity here for us to be able to tell not supported from
                // supported, but malformed.
                Some(netlink_packet::new_error(Err(decode_error_to_errno(&error)), header))
            }
            ValidationError::Permission { header, error } => {
                Some(netlink_packet::new_error(Err(error), header))
            }
        }
    }
}

/// Encapsulates `NetlinkMessage` with the credentials of the sender.
#[derive(Clone, Debug)]
pub struct NetlinkMessageWithCreds<M, C> {
    message: M,
    creds: C,
}

impl<M, C> NetlinkMessageWithCreds<M, C> {
    /// Creates a new instance.
    pub fn new(message: M, creds: C) -> Self {
        Self { message, creds }
    }
}

/// A trait abstracting a yet unvalidated netlink message.
///
/// This trait provides storage-abstraction for [`NetlinkMessageWithCreds`].
pub trait UnvalidatedNetlinkMessage {
    /// The message type in this unvalidated message.
    type Message;
    /// The credentials type required for validation.
    type Credentials;

    /// Validates permission using the specified `AccessControl` and returns
    /// the parsed message. If the permission is not granted then return an
    /// error that should be sent back to the client.
    fn validate_creds_and_get_message<PS: AccessControl<Self::Credentials>>(
        self,
        access_control: &PS,
    ) -> Result<NetlinkMessage<Self::Message>, ValidationError>;
}

impl<M, C> UnvalidatedNetlinkMessage for NetlinkMessageWithCreds<M, C>
where
    M: MaybeParsedNetlinkMessage,
    M::Message: MessageWithPermission,
{
    type Message = M::Message;
    type Credentials = C;

    fn validate_creds_and_get_message<PS: AccessControl<C>>(
        self,
        access_control: &PS,
    ) -> Result<NetlinkMessage<M::Message>, ValidationError> {
        let Self { message, creds } = self;
        let message = message.try_into_parsed().map_err(ValidationError::Parse)?;
        let permission = match &message.payload {
            NetlinkPayload::InnerMessage(msg) => msg.permission(),
            NetlinkPayload::Done(_)
            | NetlinkPayload::Error(_)
            | NetlinkPayload::Noop
            | NetlinkPayload::Overrun(_) => return Ok(message),
        };

        access_control
            .grant_assess(&creds, permission)
            .map_err(|error| ValidationError::Permission { header: message.header, error })?;
        Ok(message)
    }
}

/// A type capable of providing a concrete types used in Netlink.
pub trait NetlinkContext {
    /// The type used to represent process credentials.
    type Creds: Clone + Send + Debug;

    /// The type of [`Sender`] provided.
    type Sender<M: Clone + NetlinkSerializable + Send>: Sender<M>;

    /// The type of [`Receiver`] provided.
    type Receiver<M: Send + MessageWithPermission + NetlinkDeserializable<Error: Into<DecodeError>>>: Receiver<M, Self::Creds>;

    /// The type of an object that validates access to netlink operations.
    type AccessControl<'a>: AccessControl<Self::Creds>;
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use crate::mpsc;
    use futures::{FutureExt as _, StreamExt as _};
    use netlink_packet_core::NetlinkSerializable;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct SentMessage<M> {
        pub message: NetlinkMessage<M>,
        pub group: Option<ModernGroup>,
    }

    impl<M> SentMessage<M> {
        pub(crate) fn unicast(message: NetlinkMessage<M>) -> Self {
            Self { message, group: None }
        }

        pub(crate) fn multicast(message: NetlinkMessage<M>, group: ModernGroup) -> Self {
            Self { message, group: Some(group) }
        }
    }

    #[derive(Clone, Debug)]
    pub(crate) struct FakeSender<M> {
        sender: futures::channel::mpsc::UnboundedSender<SentMessage<M>>,
    }

    impl<M: Clone + Send + NetlinkSerializable> Sender<M> for FakeSender<M> {
        fn send(&mut self, message: NetlinkMessage<M>, group: Option<ModernGroup>) {
            self.sender
                .unbounded_send(SentMessage { message, group })
                .expect("unable to send message");
        }
    }

    pub(crate) struct FakeSenderSink<M> {
        receiver: futures::channel::mpsc::UnboundedReceiver<SentMessage<M>>,
    }

    impl<M> FakeSenderSink<M> {
        pub(crate) fn take_messages(&mut self) -> Vec<SentMessage<M>> {
            let mut messages = Vec::new();
            while let Some(msg_opt) = self.receiver.next().now_or_never() {
                match msg_opt {
                    Some(msg) => messages.push(msg),
                    None => return messages, // Stream closed.
                };
            }
            // All receiver messages that were ready were added.
            messages
        }

        pub(crate) async fn next_message(&mut self) -> SentMessage<M> {
            self.receiver.next().await.expect("receiver unexpectedly closed")
        }
    }

    pub(crate) fn fake_sender_with_sink<M>() -> (FakeSender<M>, FakeSenderSink<M>) {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        (FakeSender { sender }, FakeSenderSink { receiver })
    }

    #[derive(Default, Debug, Clone)]
    pub(crate) struct FakeCreds {
        error: Option<Errno>,
    }

    impl FakeCreds {
        pub fn with_error(error: Errno) -> Self {
            FakeCreds { error: Some(error) }
        }
    }

    #[derive(Default, Clone)]
    pub(crate) struct FakeAccessControl {}

    impl AccessControl<FakeCreds> for FakeAccessControl {
        fn grant_assess(&self, creds: &FakeCreds, _perm: Permission) -> Result<(), Errno> {
            if let Some(ref error) = creds.error { Err(*error) } else { Ok(()) }
        }
    }

    pub(crate) struct TestNetlinkContext;

    impl NetlinkContext for TestNetlinkContext {
        type Creds = FakeCreds;
        type Sender<M: Clone + NetlinkSerializable + Send> = FakeSender<M>;
        type Receiver<
            M: Send + MessageWithPermission + NetlinkDeserializable<Error: Into<DecodeError>>,
        > = mpsc::Receiver<NetlinkMessageWithCreds<NetlinkMessage<M>, Self::Creds>>;
        type AccessControl<'a> = FakeAccessControl;
    }
}
