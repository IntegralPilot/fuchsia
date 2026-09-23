// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::handle::handle_type;
use crate::responder::Responder;
use crate::{Error, Event, EventPair, Handle, OnFDomainSignals, Socket, Vmo, ordinals};
use fidl_fuchsia_fdomain as proto;
use futures::future::Either;
use futures::stream::Stream;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, ready};

/// A channel in a remote FDomain.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Channel(pub(crate) Handle);

handle_type!(Channel CHANNEL peered);

/// A message which has been read from a channel.
#[derive(Debug)]
pub struct MessageBuf {
    pub bytes: Vec<u8>,
    pub handles: Vec<HandleInfo>,
}

impl MessageBuf {
    /// Create a new [`MessageBuf`]
    pub fn new() -> Self {
        MessageBuf { bytes: Vec::new(), handles: Vec::new() }
    }

    /// Get the components of this buffer separately.
    pub fn split(self) -> (Vec<u8>, Vec<HandleInfo>) {
        (self.bytes, self.handles)
    }

    /// Make sure this buffer has room for a certain number of bytes.
    pub fn ensure_capacity_bytes(&mut self, bytes: usize) {
        self.bytes.reserve(bytes);
    }

    /// Clear out the contents of this buffer.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.handles.clear();
    }

    /// Get the byte content of this buffer.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Convert a proto ChannelMessage to a MessageBuf.
    fn from_proto(client: &Arc<crate::Client>, message: proto::ChannelMessage) -> MessageBuf {
        let proto::ChannelMessage { data, handles } = message;
        MessageBuf {
            bytes: data,
            handles: handles
                .into_iter()
                .map(|info| {
                    let handle = Handle { id: info.handle.id, client: Arc::downgrade(client) };
                    HandleInfo {
                        rights: info.rights,
                        handle: AnyHandle::from_handle(handle, info.type_),
                    }
                })
                .collect(),
        }
    }
}

/// A handle which has been read from a channel.
#[derive(Debug)]
pub struct HandleInfo {
    pub handle: AnyHandle,
    pub rights: fidl::Rights,
}

/// Sum type of all the handle types which can be read from a channel. Allows
/// the user to learn the type of a handle after it has been read.
#[derive(Debug, PartialEq)]
pub enum AnyHandle {
    Channel(Channel),
    Socket(Socket),
    Event(Event),
    EventPair(EventPair),
    Vmo(Vmo),
    Unknown(Handle, fidl::ObjectType),
}

impl AnyHandle {
    /// Construct an `AnyHandle` from a `Handle` and an object type.
    pub fn from_handle(handle: Handle, ty: fidl::ObjectType) -> AnyHandle {
        match ty {
            fidl::ObjectType::CHANNEL => AnyHandle::Channel(Channel(handle)),
            fidl::ObjectType::SOCKET => AnyHandle::Socket(Socket(handle)),
            fidl::ObjectType::EVENT => AnyHandle::Event(Event(handle)),
            fidl::ObjectType::EVENTPAIR => AnyHandle::EventPair(EventPair(handle)),
            fidl::ObjectType::VMO => AnyHandle::Vmo(Vmo(handle)),
            _ => AnyHandle::Unknown(handle, ty),
        }
    }

    /// Get an `AnyHandle` wrapping an invalid handle.
    pub fn invalid() -> AnyHandle {
        AnyHandle::Unknown(Handle::invalid(), fidl::ObjectType::NONE)
    }

    /// Check whether this handle is valid.
    pub fn is_invalid(&self) -> bool {
        match self {
            AnyHandle::Channel(h) => h.is_invalid(),
            AnyHandle::Socket(h) => h.is_invalid(),
            AnyHandle::Event(h) => h.is_invalid(),
            AnyHandle::EventPair(h) => h.is_invalid(),
            AnyHandle::Vmo(h) => h.is_invalid(),
            AnyHandle::Unknown(h, _) => h.is_invalid(),
        }
    }

    /// Get the object type for a handle.
    pub fn object_type(&self) -> fidl::ObjectType {
        match self {
            AnyHandle::Channel(_) => fidl::ObjectType::CHANNEL,
            AnyHandle::Socket(_) => fidl::ObjectType::SOCKET,
            AnyHandle::Event(_) => fidl::ObjectType::EVENT,
            AnyHandle::EventPair(_) => fidl::ObjectType::EVENTPAIR,
            AnyHandle::Vmo(_) => fidl::ObjectType::VMO,
            AnyHandle::Unknown(_, t) => *t,
        }
    }
}

impl From<AnyHandle> for Handle {
    fn from(item: AnyHandle) -> Handle {
        match item {
            AnyHandle::Channel(h) => h.into(),
            AnyHandle::Socket(h) => h.into(),
            AnyHandle::Event(h) => h.into(),
            AnyHandle::EventPair(h) => h.into(),
            AnyHandle::Vmo(h) => h.into(),
            AnyHandle::Unknown(h, _) => h,
        }
    }
}

/// Operation to perform on a handle when writing it to a channel.
pub enum HandleOp<'h> {
    Move(Handle, fidl::Rights),
    Duplicate(&'h Handle, fidl::Rights),
}

impl Channel {
    /// Reads a message from the channel.
    pub fn recv_msg(&self) -> impl Future<Output = Result<MessageBuf, Error>> + use<> {
        let client = Arc::downgrade(&self.0.client());
        let handle = self.0.proto();

        futures::future::poll_fn(move |ctx| {
            let client = client.upgrade().unwrap_or_else(|| Arc::clone(&crate::DEAD_CLIENT));
            client.poll_channel(handle, ctx, false).map(|x| {
                x.expect("Got stream termination indication from non-streaming read!")
                    .map(|x| MessageBuf::from_proto(&client, x))
            })
        })
    }

    /// Poll to try and read a channel message.
    pub fn poll_read(&self, cx: &mut Context<'_>) -> Poll<Result<MessageBuf, Error>> {
        let client = self.0.client();
        let handle = self.0.proto();

        client.poll_channel(handle, cx, false).map(|x| {
            x.expect("Got stream termination indication from non-streaming read!")
                .map(|x| MessageBuf::from_proto(&client, x))
        })
    }

    /// Poll a channel for a message to read.
    pub fn recv_from(&self, cx: &mut Context<'_>, buf: &mut MessageBuf) -> Poll<Result<(), Error>> {
        let client = self.0.client();
        match ready!(client.poll_channel(self.0.proto(), cx, false))
            .expect("Got stream termination indication from non-streaming read!")
        {
            Ok(msg) => {
                *buf = MessageBuf::from_proto(&client, msg);
                Poll::Ready(Ok(()))
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    /// Writes a message into the channel.
    pub fn write(&self, bytes: &[u8], handles: Vec<Handle>) -> Result<(), Error> {
        if bytes.len() > zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize
            || handles.len() > zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize
        {
            return Err(Error::FDomain(proto::Error::TargetError(
                fidl::Status::OUT_OF_RANGE.into_raw(),
            )));
        }

        let _ = self.write_inner(
            bytes,
            proto::Handles::Handles(handles.into_iter().map(|x| x.take_proto()).collect()),
        );
        Ok(())
    }

    /// Writes a message into the channel. Returns a future that will allow you
    /// to wait for the write to move across the FDomain connection and return
    /// with the result of the actual write call on the target.
    pub fn fdomain_write(
        &self,
        bytes: &[u8],
        handles: Vec<Handle>,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        if bytes.len() > zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize
            || handles.len() > zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize
        {
            Either::Left(async {
                Err(Error::FDomain(proto::Error::TargetError(
                    fidl::Status::OUT_OF_RANGE.into_raw(),
                )))
            })
        } else {
            Either::Right(self.write_inner(
                bytes,
                proto::Handles::Handles(handles.into_iter().map(|x| x.take_proto()).collect()),
            ))
        }
    }

    /// A future that returns when the channel is closed.
    pub fn on_closed(&self) -> OnFDomainSignals {
        OnFDomainSignals::new(&self.0, fidl::Signals::OBJECT_PEER_CLOSED)
    }

    /// Whether this handle is closed.
    pub fn is_closed(&self) -> bool {
        self.0.client.upgrade().is_none()
    }

    /// Writes a message into the channel. Optionally duplicates some of the
    /// handles rather than consuming them, and can update the handle's rights
    /// before sending.
    pub fn fdomain_write_etc<'b>(
        &self,
        bytes: &[u8],
        handles: Vec<HandleOp<'b>>,
    ) -> impl Future<Output = Result<(), Error>> + use<'b> {
        let handles = handles
            .into_iter()
            .map(|handle| match handle {
                HandleOp::Move(x, rights) => {
                    if Weak::ptr_eq(&x.client, &self.0.client) {
                        Ok(proto::HandleDisposition {
                            handle: proto::HandleOp::Move_(x.take_proto()),
                            rights,
                        })
                    } else {
                        Err(Error::ConnectionMismatch)
                    }
                }
                HandleOp::Duplicate(x, rights) => {
                    if Weak::ptr_eq(&x.client, &self.0.client) {
                        Ok(proto::HandleDisposition {
                            handle: proto::HandleOp::Duplicate(x.proto()),
                            rights,
                        })
                    } else {
                        Err(Error::ConnectionMismatch)
                    }
                }
            })
            .collect::<Result<Vec<_>, Error>>();

        let handles = if handles
            .as_ref()
            .map(|x| x.len() > zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize)
            .unwrap_or(false)
            || bytes.len() > zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize
        {
            Err(Error::FDomain(proto::Error::TargetError(fidl::Status::OUT_OF_RANGE.into_raw())))
        } else {
            handles
        };

        match handles {
            Ok(handles) => {
                Either::Left(self.write_inner(bytes, proto::Handles::Dispositions(handles)))
            }
            Err(e) => Either::Right(async move { Err(e) }),
        }
    }

    /// Writes a message into the channel.
    fn write_inner(
        &self,
        bytes: &[u8],
        handles: proto::Handles,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let data = bytes.to_vec();
        let client = self.0.client();
        let handle = self.0.proto();

        client.clear_handles_for_transfer(&handles);
        client.transaction(
            ordinals::WRITE_CHANNEL,
            proto::ChannelWriteChannelRequest { handle, data, handles },
            move |x| Responder::WriteChannel(x),
        )
    }

    /// Split this channel into a streaming reader and a writer. This is more
    /// efficient on the read side if you intend to consume all of the messages
    /// from the channel. However it will prevent you from transferring the
    /// handle in the future. It also means messages will build up in the
    /// buffer, so it may lead to memory issues if you don't intend to use the
    /// messages from the channel as fast as they come.
    pub fn stream(self) -> Result<(ChannelMessageStream, ChannelWriter), Error> {
        let (a, b, err) = self.force_stream();
        if let Some(err) = err { Err(err) } else { Ok((a, b)) }
    }

    /// Same as `stream` but will always try to generate the stream. If the
    /// error is populated, the stream should also return the error at use time,
    /// which is a more convenient way to handle the error in some cases.
    pub(crate) fn force_stream(self) -> (ChannelMessageStream, ChannelWriter, Option<Error>) {
        let err = self.0.client().start_channel_streaming(self.0.proto()).err();

        let a = Arc::new(self);
        let b = Arc::clone(&a);

        (ChannelMessageStream(a), ChannelWriter(b), err)
    }
}

/// A write-only handle to a socket.
#[derive(Debug, Clone)]
pub struct ChannelWriter(Arc<Channel>);

impl ChannelWriter {
    /// Writes a message into the channel.
    pub fn write(&self, bytes: &[u8], handles: Vec<Handle>) -> Result<(), Error> {
        self.0.write(bytes, handles)
    }

    /// Writes a message into the channel. Returns a future that will allow you
    /// to wait for the write to move across the FDomain connection and return
    /// with the result of the actual write call on the target.
    pub fn fdomain_write(
        &self,
        bytes: &[u8],
        handles: Vec<Handle>,
    ) -> impl Future<Output = Result<(), Error>> {
        self.0.fdomain_write(bytes, handles)
    }

    /// Writes a message into the channel.
    pub fn fdomain_write_etc<'b>(
        &self,
        bytes: &[u8],
        handles: Vec<HandleOp<'b>>,
    ) -> impl Future<Output = Result<(), Error>> + 'b {
        self.0.fdomain_write_etc(bytes, handles)
    }

    /// Get a reference to the inner channel.
    pub fn as_channel(&self) -> &Channel {
        &*self.0
    }
}

/// A stream of data issuing from a socket.
#[derive(Debug)]
pub struct ChannelMessageStream(Arc<Channel>);

impl ChannelMessageStream {
    /// Turn a `ChannelMessageStream` and its accompanying `ChannelWriter` back
    /// into a `Channel`.
    ///
    /// # Panics
    /// If this stream and the writer passed didn't come from the same call to
    /// `Channel::stream`, or if there is more than one writer.
    pub fn rejoin(mut self, writer: ChannelWriter) -> Channel {
        assert!(Arc::ptr_eq(&self.0, &writer.0), "Tried to join stream with wrong writer!");
        if let Some(client) = self.0.0.client.upgrade() {
            client.stop_channel_streaming(self.0.0.proto())
        }
        std::mem::drop(writer);
        let channel = std::mem::replace(&mut self.0, Arc::new(Channel(Handle::invalid())));
        Arc::try_unwrap(channel).expect("Stream pointer no longer unique!")
    }

    /// Whether this stream is closed.
    pub fn is_closed(&self) -> bool {
        let client = self.0.0.client();

        !client.channel_is_streaming(self.0.0.proto())
    }

    /// Get a reference to the inner channel.
    pub fn as_channel(&self) -> &Channel {
        &*self.0
    }
}

impl Stream for ChannelMessageStream {
    type Item = Result<MessageBuf, Error>;
    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let client = self.0.0.client();
        client
            .poll_channel(self.0.0.proto(), ctx, true)
            .map(|x| x.map(|x| x.map(|x| MessageBuf::from_proto(&client, x))))
    }
}

impl Drop for ChannelMessageStream {
    fn drop(&mut self) {
        if let Some(client) = self.0.0.client.upgrade() {
            client.stop_channel_streaming(self.0.0.proto());
        }
    }
}
