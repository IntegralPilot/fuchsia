// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::ValueError;
use fdomain_client::AsHandleRef;
use fidl_codec_fdomain::{ObjectType, Value as FidlValue};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crate::error::Result;
use crate::interpreter::IOError;

pub enum Endpoint {
    Client(fdomain_client::Channel, String),
    Server(fdomain_client::Channel, String),
    Socket(fdomain_client::Socket),
}

impl Endpoint {
    fn fdomain(&self) -> Arc<fdomain_client::Client> {
        match self {
            Endpoint::Client(h, _) => h.domain(),
            Endpoint::Server(h, _) => h.domain(),
            Endpoint::Socket(h) => h.domain(),
        }
    }

    fn object_type(&self) -> fidl::ObjectType {
        match self {
            Endpoint::Client(_, _) | Endpoint::Server(_, _) => fidl::ObjectType::CHANNEL,
            Endpoint::Socket(_) => fidl::ObjectType::SOCKET,
        }
    }

    fn endpoint_type(&self) -> Option<(EndpointType, &str)> {
        match self {
            Endpoint::Client(_, p) => Some((EndpointType::Client, p)),
            Endpoint::Server(_, p) => Some((EndpointType::Server, p)),
            Endpoint::Socket(_) => None,
        }
    }
}

#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub enum EndpointType {
    Client,
    Server,
}

impl std::fmt::Display for EndpointType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EndpointType::Client => write!(f, "client"),
            EndpointType::Server => write!(f, "server"),
        }
    }
}

impl EndpointType {
    pub(super) fn opposite(&self) -> Self {
        match self {
            EndpointType::Client => EndpointType::Server,
            EndpointType::Server => EndpointType::Client,
        }
    }

    fn construct(&self, channel: fdomain_client::Channel, protocol: String) -> Endpoint {
        match self {
            EndpointType::Client => Endpoint::Client(channel, protocol),
            EndpointType::Server => Endpoint::Server(channel, protocol),
        }
    }
}

/// As it says, either an `Endpoint` or an FDomain client. The `BadValue`
/// variant is there to make exchanging the value easier; it should never exist
/// for long and it is acceptable to panic if it is observed where not expected.
enum EndpointOrFDomain {
    Endpoint(Endpoint),
    FDomainClient(Arc<fdomain_client::Client>),
    BadValue,
}

/// Stores an actual handle along with state information about the type of handle it is.
enum HandleObject {
    Handle(fdomain_client::Handle, fidl::ObjectType),
    Endpoint(Endpoint),
    Undetermined(Arc<Mutex<EndpointOrFDomain>>),
    Defunct,
}

impl HandleObject {
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    fn fdomain(&self) -> Result<Arc<fdomain_client::Client>> {
        match self {
            HandleObject::Handle(h, _) => Ok(h.domain()),
            HandleObject::Endpoint(endpoint) => Ok(endpoint.fdomain()),
            HandleObject::Undetermined(endpoint) => match &*endpoint.lock().unwrap() {
                EndpointOrFDomain::Endpoint(endpoint) => Ok(endpoint.fdomain()),
                EndpointOrFDomain::FDomainClient(c) => Ok(Arc::clone(c)),
                EndpointOrFDomain::BadValue => unreachable!(),
            },
            HandleObject::Defunct => Err(ValueError::HandleClosed.into()),
        }
    }

    fn is_client_end(&mut self) -> bool {
        self.determine();
        matches!(self, HandleObject::Endpoint(Endpoint::Client(_, _)))
    }

    fn determine(&mut self) {
        if let HandleObject::Undetermined(this) = self {
            let mut got = this.lock().unwrap();
            if matches!(*got, EndpointOrFDomain::Endpoint(_)) {
                let EndpointOrFDomain::Endpoint(e) =
                    std::mem::replace(&mut *got, EndpointOrFDomain::BadValue)
                else {
                    unreachable!()
                };
                std::mem::drop(got);

                *self = HandleObject::Endpoint(e);
            }
        }
    }
}

/// Represents a handle that is currently in use by the interpreter. Handles are
/// single-owner things normally, but the interpreter allows multiple access to
/// them. That access is coordinated here by surrounding the handle in a lock
/// and allowing patterns for changing the handle's type in response to type
/// coercion, or stealing the handle entirely if it is to be sent over the wire.
#[derive(Clone)]
pub struct InUseHandle {
    handle: Arc<Mutex<HandleObject>>,
}

impl InUseHandle {
    /// Get the FDomain associated with this handle.
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    pub fn fdomain(&self) -> Result<Arc<fdomain_client::Client>> {
        self.handle.lock().unwrap().fdomain()
    }

    /// Create a new [`InUseHandle`] for a client end channel.
    pub fn client_end(channel: fdomain_client::Channel, identifier: String) -> Self {
        InUseHandle {
            handle: Arc::new(Mutex::new(HandleObject::Endpoint(Endpoint::Client(
                channel, identifier,
            )))),
        }
    }

    /// Get the type of this handle. Returns `None` if the handle has been
    /// consumed, or if it is unconstructed due to insufficient type
    /// information.
    pub fn object_type(&self) -> Option<fidl::ObjectType> {
        match &*self.handle.lock().unwrap() {
            HandleObject::Handle(_, ty) => Some(*ty),
            HandleObject::Endpoint(e) => Some(e.object_type()),
            HandleObject::Undetermined(s) => {
                let s = s.lock().unwrap();

                if let EndpointOrFDomain::Endpoint(s) = &*s { Some(s.object_type()) } else { None }
            }
            HandleObject::Defunct => None,
        }
    }

    /// Whether this is a client end handle.
    pub fn is_client_end(&self) -> bool {
        self.handle.lock().unwrap().is_client_end()
    }

    /// Create a new [`InUseHandle`] for a server end channel.
    pub fn server_end(channel: fdomain_client::Channel, identifier: String) -> Self {
        InUseHandle {
            handle: Arc::new(Mutex::new(HandleObject::Endpoint(Endpoint::Server(
                channel, identifier,
            )))),
        }
    }

    /// Create a new [`InUseHandle`] for an arbitrary handle.
    pub fn handle(handle: fdomain_client::Handle, ty: fidl::ObjectType) -> Self {
        InUseHandle { handle: Arc::new(Mutex::new(HandleObject::Handle(handle, ty))) }
    }

    /// Create new paired handle set. The handles could become channels or
    /// sockets depending on how they are used.
    pub fn new_endpoints(client: Arc<fdomain_client::Client>) -> (Self, Self) {
        let endpoint = Arc::new(Mutex::new(EndpointOrFDomain::FDomainClient(client)));
        (
            InUseHandle {
                handle: Arc::new(Mutex::new(HandleObject::Undetermined(Arc::clone(&endpoint)))),
            },
            InUseHandle { handle: Arc::new(Mutex::new(HandleObject::Undetermined(endpoint))) },
        )
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// If this handle is a client end channel with the given protocol, steal it
    /// and return a raw FIDL value wrapping it.
    pub fn take_client(&self, expect_proto: Option<&str>) -> Result<fdomain_client::Channel> {
        self.take_endpoint(EndpointType::Client, expect_proto)
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// If this handle is a server end channel with the given protocol, steal it
    /// and return a raw FIDL value wrapping it.
    pub fn take_server(&self, expect_proto: Option<&str>) -> Result<fdomain_client::Channel> {
        self.take_endpoint(EndpointType::Server, expect_proto)
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// If this handle is a channel with the given protocol and endpoint type, steal it
    /// and return a raw FIDL value wrapping it.
    fn take_endpoint(
        &self,
        ty: EndpointType,
        expect_proto: Option<&str>,
    ) -> Result<fdomain_client::Channel> {
        let mut this = self.handle.lock().unwrap();
        this.determine();

        match &mut *this {
            HandleObject::Handle(_, t) if *t == fidl::ObjectType::CHANNEL => {
                let HandleObject::Handle(h, _) =
                    std::mem::replace(&mut *this, HandleObject::Defunct)
                else {
                    unreachable!();
                };
                Ok(h.into())
            }
            HandleObject::Endpoint(e) => {
                let Some((got_ty, proto)) = e.endpoint_type() else {
                    return Err(ValueError::NotChannel.into());
                };
                let expect_proto_mismatched = expect_proto.filter(|x| *x != proto);
                if got_ty != ty && expect_proto_mismatched.is_some() {
                    let expect_proto = expect_proto_mismatched.unwrap();
                    return Err(ValueError::EndpointMismatch {
                        got_ty,
                        got_proto: proto.to_owned(),
                        expect_ty: ty,
                        expect_proto: expect_proto.to_owned(),
                    }
                    .into());
                } else if got_ty != ty {
                    return Err(ValueError::EndpointTypeMismatch(got_ty).into());
                } else if let Some(expect_proto) = expect_proto_mismatched {
                    return Err(ValueError::EndpointProtocolMismatch(
                        got_ty,
                        proto.to_owned(),
                        expect_proto.to_owned(),
                    )
                    .into());
                }
                let HandleObject::Endpoint(Endpoint::Server(h, _) | Endpoint::Client(h, _)) =
                    std::mem::replace(&mut *this, HandleObject::Defunct)
                else {
                    unreachable!();
                };
                Ok(h.into())
            }
            HandleObject::Undetermined(e) => {
                let Some(expect_proto) = expect_proto else {
                    return Err(ValueError::UndeterminedProtocol.into());
                };
                let mut e = e.lock().unwrap();
                let EndpointOrFDomain::FDomainClient(c) = &mut *e else { unreachable!() };
                let (a, b) = c.create_channel();
                *e = EndpointOrFDomain::Endpoint(
                    ty.opposite().construct(a, expect_proto.to_owned()),
                );
                drop(e);
                Ok(b)
            }
            HandleObject::Defunct => Err(ValueError::HandleClosed.into()),
            HandleObject::Handle(_, _) => Err(ValueError::NotAnEndpoint(ty).into()),
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// If this handle is a socket, steal it and return a raw FIDL value
    /// wrapping it.
    pub fn take_socket(&self) -> Result<FidlValue> {
        let mut this = self.handle.lock().unwrap();
        this.determine();
        match &mut *this {
            HandleObject::Handle(_, t) if *t == fidl::ObjectType::SOCKET => {
                let HandleObject::Handle(h, _) =
                    std::mem::replace(&mut *this, HandleObject::Defunct)
                else {
                    unreachable!();
                };
                Ok(FidlValue::Handle(h.into(), ObjectType::Socket, None))
            }
            HandleObject::Endpoint(Endpoint::Socket(_)) => {
                let HandleObject::Endpoint(Endpoint::Socket(h)) =
                    std::mem::replace(&mut *this, HandleObject::Defunct)
                else {
                    unreachable!();
                };
                Ok(FidlValue::Handle(h.into(), ObjectType::Socket, None))
            }
            HandleObject::Undetermined(e) => {
                let mut e = e.lock().unwrap();
                let EndpointOrFDomain::FDomainClient(c) = &mut *e else { unreachable!() };
                let (a, b) = c.create_stream_socket();
                *e = EndpointOrFDomain::Endpoint(Endpoint::Socket(a));
                drop(e);
                Ok(FidlValue::Handle(b.into(), ObjectType::Socket, None))
            }
            HandleObject::Defunct => Err(ValueError::HandleClosed.into()),
            _ => Err(ValueError::NotSocket.into()),
        }
    }

    /// Test method that verifies that this contains a socket and returns it as a socket.
    #[cfg(test)]
    pub fn unwrap_socket(self) -> fdomain_client::Socket {
        let mut h = std::mem::replace(&mut *self.handle.lock().unwrap(), HandleObject::Defunct);
        h.determine();
        let HandleObject::Endpoint(Endpoint::Socket(h)) = h else { panic!() };
        h.into()
    }

    /// If this is a channel, perform a `read_etc` operation on it
    /// asynchronously. Returns an error if it is not a channel.
    fn poll_read_channel_etc(
        &self,
        ctx: &mut Context<'_>,
    ) -> Poll<Result<fdomain_client::MessageBuf>> {
        let mut this = self.handle.lock().unwrap();
        this.determine();
        let hdl = match &*this {
            HandleObject::Endpoint(Endpoint::Client(ch, _) | Endpoint::Server(ch, _)) => ch,
            HandleObject::Handle(_, _) => {
                return Poll::Ready(Err(ValueError::RawChannelUnimplemented("reads").into()));
            }
            HandleObject::Endpoint(_) => return Poll::Ready(Err(ValueError::NotChannel.into())),
            HandleObject::Undetermined(_) => {
                return Poll::Ready(Err(ValueError::ChannelTypeUndetermined.into()));
            }
            HandleObject::Defunct => return Poll::Ready(Err(ValueError::ChannelClosed.into())),
        };

        hdl.poll_read(ctx).map_err(|e| IOError::ChannelRead(e).into())
    }

    /// If this is a channel, perform a `read_etc` operation on it within a
    /// future.
    pub async fn read_channel_etc(&self) -> Result<fdomain_client::MessageBuf> {
        futures::future::poll_fn(|ctx| self.poll_read_channel_etc(ctx)).await
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// If this is a channel, perform a `write_etc` operation on it. Returns an
    /// error if it is not a channel.
    pub async fn write_channel_etc(
        &self,
        bytes: &[u8],
        handles: Vec<fdomain_client::HandleOp<'_>>,
    ) -> Result<()> {
        let fut = {
            let mut this = self.handle.lock().unwrap();
            this.determine();
            let hdl = match &*this {
                HandleObject::Endpoint(Endpoint::Client(ch, _) | Endpoint::Server(ch, _)) => ch,
                HandleObject::Handle(_, _) => {
                    return Err(ValueError::RawChannelUnimplemented("writes").into());
                }
                HandleObject::Endpoint(_) => return Err(ValueError::NotChannel.into()),
                HandleObject::Undetermined(_) => {
                    return Err(ValueError::ChannelTypeUndetermined.into());
                }
                HandleObject::Defunct => return Err(ValueError::ChannelClosed.into()),
            };

            hdl.fdomain_write_etc(bytes, handles)
        };
        fut.await.map_err(|e| IOError::ChannelWrite(e).into())
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// Get an ID for this handle (the raw handle number). Fails if the handle
    /// has been stolen.
    pub fn id(&self) -> Result<u32> {
        let mut this = self.handle.lock().unwrap();
        this.determine();
        match &*this {
            HandleObject::Endpoint(Endpoint::Client(ch, _) | Endpoint::Server(ch, _)) => {
                Ok(ch.u32_id())
            }
            HandleObject::Endpoint(Endpoint::Socket(s)) => Ok(s.u32_id()),
            HandleObject::Undetermined(_) => Err(ValueError::NoHandleID.into()),
            HandleObject::Handle(h, _) => Ok(h.u32_id()),
            HandleObject::Defunct => Err(ValueError::HandleClosed.into()),
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// If this handle is a client, get the name of the protocol it is a client
    /// for if known.
    pub fn get_client_protocol(&self) -> Result<String> {
        let mut this = self.handle.lock().unwrap();
        this.determine();
        match &*this {
            HandleObject::Endpoint(Endpoint::Client(_, proto)) => Ok(proto.clone()),
            HandleObject::Handle(_, _)
            | HandleObject::Endpoint(_)
            | HandleObject::Undetermined(_) => {
                Err(ValueError::NotAnEndpoint(EndpointType::Client).into())
            }
            HandleObject::Defunct => Err(ValueError::HandleClosed.into()),
        }
    }
}

impl std::fmt::Display for InUseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut this = self.handle.lock().unwrap();
        this.determine();
        match &*this {
            HandleObject::Endpoint(Endpoint::Client(_, proto)) => write!(f, "ClientEnd<{proto}>"),
            HandleObject::Endpoint(Endpoint::Server(_, proto)) => write!(f, "ServerEnd<{proto}>"),
            HandleObject::Undetermined(_) => write!(f, "<unknown>"),
            HandleObject::Defunct => write!(f, "<invalid handle>"),
            HandleObject::Handle(_, ty) => write!(f, "<{ty:?}>"),
            HandleObject::Endpoint(Endpoint::Socket(_)) => {
                write!(f, "<{:?}>", fidl::ObjectType::SOCKET)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use fuchsia_async as fasync;
    use futures::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[fuchsia::test]
    async fn coerce() {
        let client = fdomain_local::local_client_empty();
        let (a, b) = InUseHandle::new_endpoints(Arc::clone(&client));
        let a = a.take_server(Some("test_proto")).unwrap();
        assert_eq!("test_proto", &b.get_client_protocol().unwrap());
        let test_str =
            b"Alas that we are themepark animatronics, and our existence is inherently whimsical";
        a.fdomain_write_etc(test_str, vec![]).await.unwrap();
        let test_buf = b.read_channel_etc().await.unwrap();
        assert_eq!(test_str, test_buf.bytes());
        assert!(test_buf.handles.is_empty());
    }

    #[fuchsia::test]
    async fn coerce_socket() {
        let client = fdomain_local::local_client_empty();
        let (a, b) = InUseHandle::new_endpoints(Arc::clone(&client));
        let FidlValue::Handle(a, a_ty, _) = a.take_socket().unwrap() else {
            panic!();
        };
        assert_eq!(ObjectType::Socket, a_ty);
        const TEST_STR: &[u8] = b"Why were we programmed to get bored anyway?";
        let mut a = fdomain_client::Socket::from(a);
        fasync::Task::spawn(async move {
            a.write_all(TEST_STR).await.unwrap();
        })
        .detach();

        let FidlValue::Handle(b, b_ty, _) = b.take_socket().unwrap() else {
            panic!();
        };
        let mut b = fdomain_client::Socket::from(b);
        assert_eq!(ObjectType::Socket, b_ty);
        let mut buf = Vec::with_capacity(TEST_STR.len());
        let got = b.read_to_end(&mut buf).await.unwrap();
        assert_eq!(TEST_STR.len(), got);
        assert_eq!(TEST_STR, &buf);
    }
}
