// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::{AsHandleRef, Peered, SocketOpts};
use fidl_fuchsia_fdomain as proto;

/// This is implemented on the `fidl::*` objects for every type of handle FDomain
/// supports. It essentially makes the handle object a responder to a stream of
/// [`HandleOperation`]s.
pub trait HandleType:
    Sync + Sized + Into<fidl::NullableHandle> + fidl::AsHandleRef + 'static
{
    /// This should be the handle type corresponding to the implementing handle.
    /// We use this to generalize some of the error reporting in this trait.
    fn object_type(&self) -> fidl::ObjectType;

    /// Returns `Ok` if this handle is the given type, `Err` otherwise.
    fn expected_type(&self, expected_type: fidl::ObjectType) -> Result<(), proto::Error> {
        if expected_type == self.object_type() {
            Ok(())
        } else {
            Err(proto::Error::WrongHandleType(proto::WrongHandleType {
                expected: expected_type,
                got: self.object_type(),
            }))
        }
    }

    /// Implements [`HandleOperation::SocketDisposition`].
    fn socket_disposition(
        &self,
        _disposition: proto::SocketDisposition,
        _disposition_peer: proto::SocketDisposition,
    ) -> Result<(), proto::Error> {
        self.expected_type(fidl::ObjectType::SOCKET)
    }

    /// Implements [`HandleOperation::ReadSocket`].
    fn read_socket(&self, _max_bytes: u64) -> Result<Option<Vec<u8>>, proto::Error> {
        Err(self.expected_type(fidl::ObjectType::SOCKET).unwrap_err())
    }

    /// Implements [`HandleOperation::ReadChannel`].
    fn read_channel(&self) -> Result<Option<fidl::MessageBufEtc>, proto::Error> {
        Err(self.expected_type(fidl::ObjectType::CHANNEL).unwrap_err())
    }

    /// Implements [`HandleOperation::WriteSocket`].
    fn write_socket(&self, _data: &[u8]) -> Result<usize, proto::Error> {
        Err(self.expected_type(fidl::ObjectType::SOCKET).unwrap_err())
    }

    /// Implements [`HandleOperation::WriteChannel`].
    fn write_channel(
        &self,
        _data: &[u8],
        _handles: &mut Vec<fidl::HandleDisposition<'static>>,
    ) -> Option<Result<(), proto::WriteChannelError>> {
        Some(Err(proto::WriteChannelError::Error(
            self.expected_type(fidl::ObjectType::CHANNEL).unwrap_err(),
        )))
    }

    /// Implements reading from a VMO.
    fn read_vmo(&self, _offset: u64, _size: u64) -> Result<Vec<u8>, proto::Error> {
        Err(self.expected_type(fidl::ObjectType::VMO).unwrap_err())
    }

    /// Implements writing to a VMO.
    fn write_vmo(&self, _offset: u64, _data: &[u8]) -> Result<(), proto::Error> {
        self.expected_type(fidl::ObjectType::VMO)
    }

    /// Implements getting the size of a VMO.
    fn get_vmo_size(&self) -> Result<u64, proto::Error> {
        Err(self.expected_type(fidl::ObjectType::VMO).unwrap_err())
    }

    /// Implements setting the size of a VMO.
    fn set_vmo_size(&self, _size: u64) -> Result<(), proto::Error> {
        self.expected_type(fidl::ObjectType::VMO)
    }

    /// Implements getting the stream size of a VMO.
    fn get_vmo_stream_size(&self) -> Result<u64, proto::Error> {
        Err(self.expected_type(fidl::ObjectType::VMO).unwrap_err())
    }

    /// Implements setting the stream size of a VMO.
    fn set_vmo_stream_size(&self, _size: u64) -> Result<(), proto::Error> {
        self.expected_type(fidl::ObjectType::VMO)
    }
}

impl HandleType for fidl::Socket {
    fn object_type(&self) -> fidl::ObjectType {
        fidl::ObjectType::SOCKET
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn socket_disposition(
        &self,
        _disposition: proto::SocketDisposition,
        _disposition_peer: proto::SocketDisposition,
    ) -> Result<(), proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    #[cfg(target_os = "fuchsia")]
    fn socket_disposition(
        &self,
        disposition: proto::SocketDisposition,
        disposition_peer: proto::SocketDisposition,
    ) -> Result<(), proto::Error> {
        fn map_disposition(
            disposition: proto::SocketDisposition,
        ) -> Result<Option<zx::SocketWriteDisposition>, proto::Error> {
            match disposition {
                proto::SocketDisposition::NoChange => Ok(None),
                proto::SocketDisposition::WriteDisabled => {
                    Ok(Some(zx::SocketWriteDisposition::Disabled))
                }
                proto::SocketDisposition::WriteEnabled => {
                    Ok(Some(zx::SocketWriteDisposition::Enabled))
                }
                disposition => {
                    Err(proto::Error::SocketDispositionUnknown(proto::SocketDispositionUnknown {
                        disposition,
                    }))
                }
            }
        }

        self.set_disposition(map_disposition(disposition)?, map_disposition(disposition_peer)?)
            .map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    fn read_socket(&self, max_bytes: u64) -> Result<Option<Vec<u8>>, proto::Error> {
        let max_bytes = max_bytes.try_into().unwrap_or(usize::MAX);
        let mut buf = Vec::with_capacity(max_bytes);
        // SAFETY: We've derived this pointer and length from a vec in a way
        // that should guarantee they are valid.
        #[cfg(target_os = "fuchsia")]
        let read = unsafe {
            let buf_ptr = buf.spare_capacity_mut();
            let buf_ptr = buf_ptr.as_mut_ptr().cast::<u8>();
            self.read_raw(buf_ptr, max_bytes)
        };
        #[cfg(not(target_os = "fuchsia"))]
        let read = {
            buf.resize(max_bytes, 0);
            self.read(&mut buf)
        };
        match read {
            Ok(size) => {
                // SAFETY: The `read_raw` call should guarantee that this many
                // bytes in the capacity have now been initialized.
                #[cfg(target_os = "fuchsia")]
                unsafe {
                    buf.set_len(size);
                }
                #[cfg(not(target_os = "fuchsia"))]
                buf.truncate(size);
                Ok(Some(buf))
            }
            Err(fidl::Status::SHOULD_WAIT) => Ok(None),
            Err(other) => Err(proto::Error::TargetError(other.into_raw())),
        }
    }

    fn write_socket(&self, data: &[u8]) -> Result<usize, proto::Error> {
        let mut wrote = 0;
        loop {
            match self.write(&data[wrote..]) {
                Ok(count) => {
                    wrote += count;

                    if wrote >= data.len() {
                        break Ok(wrote);
                    }
                }
                Err(fidl::Status::SHOULD_WAIT) => break Ok(wrote),

                Err(other) => break Err(proto::Error::TargetError(other.into_raw())),
            }
        }
    }
}

impl HandleType for fidl::Channel {
    fn object_type(&self) -> fidl::ObjectType {
        fidl::ObjectType::CHANNEL
    }

    fn read_channel(&self) -> Result<Option<fidl::MessageBufEtc>, proto::Error> {
        let mut buf = fidl::MessageBufEtc::new();
        match self.read_etc(&mut buf) {
            Err(fidl::Status::SHOULD_WAIT) => Ok(None),
            other => other.map(|_| Some(buf)).map_err(|e| proto::Error::TargetError(e.into_raw())),
        }
    }

    fn write_channel(
        &self,
        data: &[u8],
        handles: &mut Vec<fidl::HandleDisposition<'static>>,
    ) -> Option<Result<(), proto::WriteChannelError>> {
        match self.write_etc(data, handles) {
            Ok(()) => Some(Ok(())),
            Err(fidl::Status::SHOULD_WAIT) => None,
            Err(other) => {
                if handles.iter().any(|x| x.result.is_err()) {
                    Some(Err(proto::WriteChannelError::OpErrors(
                        handles
                            .iter_mut()
                            .map(|x| {
                                x.result
                                    .err()
                                    .map(|e| Box::new(proto::Error::TargetError(e.into_raw())))
                            })
                            .collect(),
                    )))
                } else {
                    Some(Err(proto::WriteChannelError::Error(proto::Error::TargetError(
                        other.into_raw(),
                    ))))
                }
            }
        }
    }
}

impl HandleType for fidl::EventPair {
    fn object_type(&self) -> fidl::ObjectType {
        fidl::ObjectType::EVENTPAIR
    }
}

impl HandleType for fidl::Event {
    fn object_type(&self) -> fidl::ObjectType {
        fidl::ObjectType::EVENT
    }
}

impl HandleType for fidl::Vmo {
    fn object_type(&self) -> fidl::ObjectType {
        fidl::ObjectType::VMO
    }

    #[cfg(target_os = "fuchsia")]
    fn read_vmo(&self, offset: u64, size: u64) -> Result<Vec<u8>, proto::Error> {
        self.read_to_vec::<u8>(offset, size).map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn read_vmo(&self, _offset: u64, _size: u64) -> Result<Vec<u8>, proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    #[cfg(target_os = "fuchsia")]
    fn write_vmo(&self, offset: u64, data: &[u8]) -> Result<(), proto::Error> {
        self.write(data, offset).map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn write_vmo(&self, _offset: u64, _data: &[u8]) -> Result<(), proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    #[cfg(target_os = "fuchsia")]
    fn get_vmo_size(&self) -> Result<u64, proto::Error> {
        self.get_size().map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn get_vmo_size(&self) -> Result<u64, proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    #[cfg(target_os = "fuchsia")]
    fn set_vmo_size(&self, size: u64) -> Result<(), proto::Error> {
        self.set_size(size).map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn set_vmo_size(&self, _size: u64) -> Result<(), proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    #[cfg(target_os = "fuchsia")]
    fn get_vmo_stream_size(&self) -> Result<u64, proto::Error> {
        self.get_stream_size().map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn get_vmo_stream_size(&self) -> Result<u64, proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    #[cfg(target_os = "fuchsia")]
    fn set_vmo_stream_size(&self, size: u64) -> Result<(), proto::Error> {
        self.set_stream_size(size).map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn set_vmo_stream_size(&self, _size: u64) -> Result<(), proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }
}

pub struct Unknown(pub fidl::NullableHandle, pub fidl::ObjectType);

impl Into<fidl::NullableHandle> for Unknown {
    fn into(self) -> fidl::NullableHandle {
        self.0
    }
}

impl fidl::AsHandleRef for Unknown {
    fn as_handle_ref(&self) -> fidl::HandleRef<'_> {
        self.0.as_handle_ref()
    }
}

impl HandleType for Unknown {
    fn object_type(&self) -> fidl::ObjectType {
        self.1
    }
}

pub enum AnyHandle {
    Socket(fidl::Socket),
    EventPair(fidl::EventPair),
    Event(fidl::Event),
    Channel(fidl::Channel),
    Vmo(fidl::Vmo),
    Unknown(Unknown),
}

/// Whether a socket is a datagram socket.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum IsDatagramSocket {
    NotDatagram,
    Datagram,
    Unknown,
}

impl IsDatagramSocket {
    pub fn is_datagram(&self) -> bool {
        matches!(self, IsDatagramSocket::Datagram)
    }
}

impl AnyHandle {
    /// Signals that indicate we should do write processing on a handle.
    pub(crate) fn write_signals(&self) -> fidl::Signals {
        let sock_signals = if let AnyHandle::Socket(_) = self {
            fidl::Signals::SOCKET_WRITE_DISABLED
        } else {
            fidl::Signals::empty()
        };

        sock_signals | fidl::Signals::OBJECT_WRITABLE | fidl::Signals::OBJECT_PEER_CLOSED
    }

    /// Signals that indicate we should do write processing on a handle.
    pub(crate) fn read_signals(&self) -> fidl::Signals {
        let sock_signals = if let AnyHandle::Socket(_) = self {
            fidl::Signals::SOCKET_PEER_WRITE_DISABLED
        } else {
            fidl::Signals::empty()
        };

        sock_signals | fidl::Signals::OBJECT_READABLE | fidl::Signals::OBJECT_PEER_CLOSED
    }

    /// Whether this is a datagram socket.
    pub(crate) fn is_datagram_socket(&self) -> IsDatagramSocket {
        let AnyHandle::Socket(socket) = self else {
            return IsDatagramSocket::NotDatagram;
        };

        let Ok(info) = socket.info() else {
            return IsDatagramSocket::Unknown;
        };

        if info.options == SocketOpts::DATAGRAM {
            IsDatagramSocket::Datagram
        } else {
            IsDatagramSocket::NotDatagram
        }
    }

    /// zx_handle_duplicate but preserving our metadata.
    pub fn duplicate(&self, rights: fidl::Rights) -> Result<AnyHandle, proto::Error> {
        let handle = self
            .as_handle_ref()
            .duplicate_handle(rights)
            .map_err(|e| proto::Error::TargetError(e.into_raw()))?;

        Ok(match self {
            AnyHandle::Socket(_) => AnyHandle::Socket(fidl::Socket::from(handle)),
            AnyHandle::EventPair(_) => AnyHandle::EventPair(fidl::EventPair::from(handle)),
            AnyHandle::Event(_) => AnyHandle::Event(fidl::Event::from(handle)),
            AnyHandle::Channel(_) => AnyHandle::Channel(fidl::Channel::from(handle)),
            AnyHandle::Vmo(_) => AnyHandle::Vmo(fidl::Vmo::from(handle)),
            AnyHandle::Unknown(Unknown(_, ty)) => AnyHandle::Unknown(Unknown(handle, *ty)),
        })
    }

    /// zx_handle_replace but preserving our metadata.
    #[cfg(not(target_os = "fuchsia"))]
    pub fn replace(self, _rights: fidl::Rights) -> Result<AnyHandle, proto::Error> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    /// zx_handle_replace but preserving our metadata.
    #[cfg(target_os = "fuchsia")]
    pub fn replace(self, rights: fidl::Rights) -> Result<AnyHandle, proto::Error> {
        Ok(match self {
            AnyHandle::Socket(h) => AnyHandle::Socket(
                h.replace_handle(rights).map_err(|e| proto::Error::TargetError(e.into_raw()))?,
            ),
            AnyHandle::EventPair(h) => AnyHandle::EventPair(
                h.replace_handle(rights).map_err(|e| proto::Error::TargetError(e.into_raw()))?,
            ),
            AnyHandle::Event(h) => AnyHandle::Event(
                h.replace_handle(rights).map_err(|e| proto::Error::TargetError(e.into_raw()))?,
            ),
            AnyHandle::Channel(h) => AnyHandle::Channel(
                h.replace_handle(rights).map_err(|e| proto::Error::TargetError(e.into_raw()))?,
            ),
            AnyHandle::Vmo(h) => AnyHandle::Vmo(
                h.replace_handle(rights).map_err(|e| proto::Error::TargetError(e.into_raw()))?,
            ),
            AnyHandle::Unknown(Unknown(h, ty)) => AnyHandle::Unknown(Unknown(
                h.replace_handle(rights).map_err(|e| proto::Error::TargetError(e.into_raw()))?,
                ty,
            )),
        })
    }

    pub fn signal(&self, clear: fidl::Signals, set: fidl::Signals) -> Result<(), proto::Error> {
        match self {
            AnyHandle::Socket(s) => s.signal(clear, set),
            AnyHandle::EventPair(e) => e.signal(clear, set),
            AnyHandle::Event(e) => e.signal(clear, set),
            AnyHandle::Channel(c) => c.signal(clear, set),
            AnyHandle::Vmo(v) => v.as_handle_ref().signal(clear, set),
            AnyHandle::Unknown(u) => u.0.signal(clear, set),
        }
        .map_err(|e| proto::Error::TargetError(e.into_raw()))
    }

    pub fn signal_peer(
        &self,
        clear: fidl::Signals,
        set: fidl::Signals,
    ) -> Result<(), proto::Error> {
        // TODO: Rust is being helpful and only letting us signal peers for
        // things we know have them, but we'd really like to just make the
        // syscall and get it to report whatever error it will. Especially for
        // `Unknown`, which may well have peers the type system just isn't aware
        // of.
        match self {
            AnyHandle::Socket(h) => h.signal_peer(clear, set),
            AnyHandle::EventPair(h) => h.signal_peer(clear, set),
            AnyHandle::Event(_) => Err(fidl::Status::INVALID_ARGS),
            AnyHandle::Channel(h) => h.signal_peer(clear, set),
            AnyHandle::Vmo(_) => Err(fidl::Status::INVALID_ARGS),
            AnyHandle::Unknown(_) => Err(fidl::Status::INVALID_ARGS),
        }
        .map_err(|e| proto::Error::TargetError(e.into_raw()))
    }
}

impl From<fidl::Channel> for AnyHandle {
    fn from(other: fidl::Channel) -> AnyHandle {
        AnyHandle::Channel(other)
    }
}

impl From<fidl::Socket> for AnyHandle {
    fn from(other: fidl::Socket) -> AnyHandle {
        AnyHandle::Socket(other)
    }
}

impl From<fidl::EventPair> for AnyHandle {
    fn from(other: fidl::EventPair) -> AnyHandle {
        AnyHandle::EventPair(other)
    }
}

impl From<fidl::Event> for AnyHandle {
    fn from(other: fidl::Event) -> AnyHandle {
        AnyHandle::Event(other)
    }
}

impl From<fidl::Vmo> for AnyHandle {
    fn from(other: fidl::Vmo) -> AnyHandle {
        AnyHandle::Vmo(other)
    }
}

macro_rules! impl_method {
    ($this:ident => $h:ident . $meth:ident ( $($args:tt)* )) => {
        match $this {
            AnyHandle::Socket($h) => $h.$meth($($args)*),
            AnyHandle::EventPair($h) => $h.$meth($($args)*),
            AnyHandle::Event($h) => $h.$meth($($args)*),
            AnyHandle::Channel($h) => $h.$meth($($args)*),
            AnyHandle::Vmo($h) => $h.$meth($($args)*),
            AnyHandle::Unknown($h) => $h.$meth($($args)*),
        }
    };
}

impl HandleType for AnyHandle {
    fn object_type(&self) -> fidl::ObjectType {
        impl_method!(self => h.object_type())
    }

    fn socket_disposition(
        &self,
        disposition: proto::SocketDisposition,
        disposition_peer: proto::SocketDisposition,
    ) -> Result<(), proto::Error> {
        impl_method!(self => h.socket_disposition(disposition, disposition_peer))
    }

    fn read_socket(&self, max_bytes: u64) -> Result<Option<Vec<u8>>, proto::Error> {
        impl_method!(self => h.read_socket(max_bytes))
    }

    fn read_channel(&self) -> Result<Option<fidl::MessageBufEtc>, proto::Error> {
        impl_method!(self => h.read_channel())
    }

    fn write_socket(&self, data: &[u8]) -> Result<usize, proto::Error> {
        impl_method!(self => h.write_socket(data))
    }

    fn write_channel(
        &self,
        data: &[u8],
        handles: &mut Vec<fidl::HandleDisposition<'static>>,
    ) -> Option<Result<(), proto::WriteChannelError>> {
        impl_method!(self => h.write_channel(data, handles))
    }

    fn read_vmo(&self, offset: u64, size: u64) -> Result<Vec<u8>, proto::Error> {
        impl_method!(self => h.read_vmo(offset, size))
    }

    fn write_vmo(&self, offset: u64, data: &[u8]) -> Result<(), proto::Error> {
        impl_method!(self => h.write_vmo(offset, data))
    }

    fn get_vmo_size(&self) -> Result<u64, proto::Error> {
        impl_method!(self => h.get_vmo_size())
    }

    fn set_vmo_size(&self, size: u64) -> Result<(), proto::Error> {
        impl_method!(self => h.set_vmo_size(size))
    }

    fn get_vmo_stream_size(&self) -> Result<u64, proto::Error> {
        impl_method!(self => h.get_vmo_stream_size())
    }

    fn set_vmo_stream_size(&self, size: u64) -> Result<(), proto::Error> {
        impl_method!(self => h.set_vmo_stream_size(size))
    }
}

impl fidl::AsHandleRef for AnyHandle {
    fn as_handle_ref(&self) -> fidl::HandleRef<'_> {
        impl_method!(self => h.as_handle_ref())
    }
}

impl Into<fidl::NullableHandle> for AnyHandle {
    fn into(self) -> fidl::NullableHandle {
        impl_method!(self => h.into())
    }
}
