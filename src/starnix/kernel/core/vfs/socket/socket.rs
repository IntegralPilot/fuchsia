// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{
    NetlinkFamily, QipcrtrSocket, SocketAddress, SocketBpfState, SocketDomain, SocketFile,
    SocketMessageFlags, SocketProtocol, SocketShutdownFlags, SocketType, UnixSocket, VsockSocket,
    ZxioBackedSocket, new_netlink_socket,
};
use crate::mm::MemoryAccessorExt;
use crate::security;
use crate::syscalls::time::TimeValPtr;
use crate::task::{CurrentTask, EventHandler, WaitCanceler, Waiter};
use crate::vfs::buffers::{AncillaryData, InputBuffer, MessageReadInfo, OutputBuffer};
use crate::vfs::{DowncastedFile, FileHandle, FileObject, FsNodeHandle};
use starnix_logging::track_stub;
use starnix_sync::{LockDepMutex, SocketStateLock};
use starnix_syscalls::{SyscallArg, SyscallResult};
use starnix_types::time::{duration_from_timeval, timeval_from_duration};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::as_any::AsAny;
use starnix_uapi::auth::CAP_NET_RAW;
use starnix_uapi::errors::{ENOTTY, Errno};
use starnix_uapi::user_address::MappingMultiArchUserRef;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    SO_DOMAIN, SO_PROTOCOL, SO_RCVTIMEO, SO_SNDTIMEO, SO_TYPE, SOL_SOCKET, errno, error, uapi,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use zerocopy::FromBytes;

pub const DEFAULT_LISTEN_BACKLOG: usize = 1024;

/// TODO(https://fxbug.dev/477273398"): These come from Android, and are currently stubbed out.
const SO_ANDROID_DROP_REASON: u32 = 0xAD01D01;
const ANDROID_DROP_REASON_NONE: u64 = 0;

pub trait SocketOps: Send + Sync + AsAny {
    /// Returns the domain, type and protocol of the socket. This is only used for socket that are
    /// build without previous knowledge of this information, and can be ignored if all sockets are
    /// build with it.
    fn get_socket_info(&self) -> Result<(SocketDomain, SocketType, SocketProtocol), Errno> {
        // This should not be used by most socket type that are created with their domain, type and
        // protocol.
        error!(EINVAL)
    }

    /// Connect the `socket` to the listening `peer`. On success
    /// a new socket is created and added to the accept queue.
    fn connect(
        &self,
        socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno>;

    /// Start listening at the bound address for `connect` calls.
    fn listen(&self, socket: &Socket, backlog: i32, credentials: uapi::ucred) -> Result<(), Errno>;

    /// Returns the eariest socket on the accept queue of this
    /// listening socket. Returns EAGAIN if the queue is empty.
    fn accept(&self, socket: &Socket, current_task: &CurrentTask) -> Result<SocketHandle, Errno>;

    /// Binds this socket to a `socket_address`.
    ///
    /// Returns an error if the socket could not be bound.
    fn bind(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno>;

    /// Reads the specified number of bytes from the socket, if possible.
    ///
    /// # Parameters
    /// - `task`: The task to which the user buffers belong (i.e., the task to which the read bytes
    ///           are written.
    /// - `data`: The buffers to write the read data into.
    ///
    /// Returns the number of bytes that were written to the user buffers, as well as any ancillary
    /// data associated with the read messages.
    fn read(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno>;

    /// Writes the data in the provided user buffers to this socket.
    ///
    /// # Parameters
    /// - `task`: The task to which the user buffers belong, used to read the memory.
    /// - `data`: The data to write to the socket.
    /// - `ancillary_data`: Optional ancillary data (a.k.a., control message) to write.
    ///
    /// Advances the iterator to indicate how much was actually written.
    fn write(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        dest_address: &mut Option<SocketAddress>,
        ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno>;

    /// Queues an asynchronous wait for the specified `events`
    /// on the `waiter`. Note that no wait occurs until a
    /// wait functions is called on the `waiter`.
    ///
    /// # Parameters
    /// - `waiter`: The Waiter that can be waited on, for example by
    ///             calling Waiter::wait_until.
    /// - `events`: The events that will trigger the waiter to wake up.
    /// - `handler`: A handler that will be called on wake-up.
    /// Returns a WaitCanceler that can be used to cancel the wait.
    fn wait_async(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler;

    /// Return the events that are currently active on the `socket`.
    fn query_events(&self, socket: &Socket, current_task: &CurrentTask) -> Result<FdEvents, Errno>;

    /// Shuts down this socket according to how, preventing any future reads and/or writes.
    ///
    /// Used by the shutdown syscalls.
    fn shutdown(&self, socket: &Socket, how: SocketShutdownFlags) -> Result<(), Errno>;

    /// Close this socket.
    ///
    /// Called by SocketFile when the file descriptor that is holding this
    /// socket is closed.
    ///
    /// Close differs from shutdown in two ways. First, close will call
    /// mark_peer_closed_with_unread_data if this socket has unread data,
    /// which changes how read() behaves on that socket. Second, close
    /// transitions the internal state of this socket to Closed, which breaks
    /// the reference cycle that exists in the connected state.
    fn close(&self, current_task: &CurrentTask, socket: &Socket);

    /// Returns the name of this socket.
    ///
    /// The name is derived from the address and domain. A socket
    /// will always have a name, even if it is not bound to an address.
    fn getsockname(&self, socket: &Socket) -> Result<SocketAddress, Errno>;

    /// Returns the name of the peer of this socket, if such a peer exists.
    ///
    /// Returns an error if the socket is not connected.
    fn getpeername(&self, socket: &Socket) -> Result<SocketAddress, Errno>;

    /// Sets socket-specific options.
    fn setsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _level: u32,
        _optname: u32,
        _optval: SockOptValue,
    ) -> Result<(), Errno> {
        error!(ENOPROTOOPT)
    }

    /// Retrieves socket-specific options.
    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _level: u32,
        _optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        error!(ENOPROTOOPT)
    }

    /// Implements ioctl.
    fn ioctl(
        &self,
        _socket: &Socket,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _request: u32,
        _arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        error!(ENOTTY)
    }

    /// Return a handle that allows access to this file descritor through the zxio protocols.
    ///
    /// If None is returned, the file will be proxied.
    fn to_handle(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        Ok(None)
    }
}

/// A `Socket` represents one endpoint of a bidirectional communication channel.
pub struct Socket {
    pub(super) ops: Box<dyn SocketOps>,

    /// The domain of this socket.
    pub domain: SocketDomain,

    /// The type of this socket.
    pub socket_type: SocketType,

    /// The protocol of this socket.
    pub protocol: SocketProtocol,

    state: LockDepMutex<SocketState, SocketStateLock>,

    /// Security module state associated with this socket. Note that the socket's security label is
    /// applied to the associated `fs_node`.
    pub security: security::SocketState,
}

#[derive(Default)]
struct SocketState {
    /// The value of SO_RCVTIMEO.
    receive_timeout: Option<zx::MonotonicDuration>,

    /// The value for SO_SNDTIMEO.
    send_timeout: Option<zx::MonotonicDuration>,

    /// Reference to the [`crate::vfs::FsNode`] to which this `Socket` is attached.
    /// `None` until the `Socket` is wrapped into a [`crate::vfs::FileObject`] (e.g. while it is
    /// still held in a listen queue).
    fs_node: Option<FsNodeHandle>,

    /// Socket state passed to eBPF programs in `bpf_sock.state` field.
    /// TODO(https://fxbug.dev/549754529): State tracking logic is incomplete
    /// for TCP sockets. It tracks only some states correctly.
    bpf_state: SocketBpfState,
}

pub type SocketHandle = Arc<Socket>;

#[derive(Clone)]
pub enum SocketPeer {
    Handle(SocketHandle),
    Address(SocketAddress),
}

// `resolve_protocol()` returns the protocol that should be used for a new
// socket. `socket()` allows `protocol` parameter to be set 0, in which case the
// protocol defaults to TCP or UDP depending on the specified `socket_type`.
fn resolve_protocol(
    domain: SocketDomain,
    socket_type: SocketType,
    protocol: SocketProtocol,
) -> SocketProtocol {
    if domain.is_inet() && protocol.as_raw() == 0 {
        match socket_type {
            SocketType::Stream => SocketProtocol::TCP,
            SocketType::Datagram => SocketProtocol::UDP,
            _ => protocol,
        }
    } else {
        protocol
    }
}

fn create_socket_ops(
    current_task: &CurrentTask,
    domain: SocketDomain,
    socket_type: SocketType,
    protocol: SocketProtocol,
) -> Result<Box<dyn SocketOps>, Errno> {
    match domain {
        SocketDomain::Unix => Ok(Box::new(UnixSocket::new(socket_type))),
        SocketDomain::Vsock => Ok(Box::new(VsockSocket::new(socket_type))),
        SocketDomain::Inet | SocketDomain::Inet6 => {
            // Follow Linux, and require CAP_NET_RAW to create raw sockets.
            // See https://man7.org/linux/man-pages/man7/raw.7.html.
            if socket_type == SocketType::Raw {
                security::check_task_capable(current_task, CAP_NET_RAW)?;
            }
            Ok(Box::new(ZxioBackedSocket::new(current_task, domain, socket_type, protocol)?))
        }
        SocketDomain::Netlink => {
            let netlink_family = NetlinkFamily::from_raw(protocol.as_raw());
            new_netlink_socket(current_task.kernel(), socket_type, netlink_family)
        }
        SocketDomain::Packet => {
            // Follow Linux, and require CAP_NET_RAW to create packet sockets.
            // See https://man7.org/linux/man-pages/man7/packet.7.html.
            security::check_task_capable(current_task, CAP_NET_RAW)?;
            Ok(Box::new(ZxioBackedSocket::new(current_task, domain, socket_type, protocol)?))
        }
        SocketDomain::Key => {
            track_stub!(
                TODO("https://fxbug.dev/323365389"),
                "Returning a UnixSocket instead of a KeySocket"
            );
            Ok(Box::new(UnixSocket::new(SocketType::Datagram)))
        }
        SocketDomain::Qipcrtr => Ok(Box::new(QipcrtrSocket::new(socket_type))),
    }
}

#[derive(Debug)]
pub enum SockOptValue {
    Value(Vec<u8>),
    User(UserBuffer),
}

impl From<Vec<u8>> for SockOptValue {
    fn from(buffer: Vec<u8>) -> Self {
        Self::Value(buffer)
    }
}

impl From<UserBuffer> for SockOptValue {
    fn from(buffer: UserBuffer) -> Self {
        Self::User(buffer)
    }
}

impl SockOptValue {
    pub fn len(&self) -> usize {
        match self {
            Self::Value(buffer) => buffer.len(),
            Self::User(user_buffer) => user_buffer.length,
        }
    }

    pub fn read<T: FromBytes>(&self, current_task: &CurrentTask) -> Result<T, Errno> {
        match self {
            Self::Value(buffer) => {
                T::read_from_prefix(&buffer).map_err(|_| errno!(EINVAL)).map(|(v, _)| v)
            }
            Self::User(user_buffer) => {
                current_task.read_object::<T>(user_buffer.clone().try_into()?)
            }
        }
    }

    pub fn read_bytes(
        &self,
        current_task: &CurrentTask,
        max_bytes: usize,
    ) -> Result<Vec<u8>, Errno> {
        match self {
            Self::Value(buffer) => {
                let bytes = std::cmp::min(max_bytes, buffer.len());
                Ok(buffer[..bytes].to_owned())
            }
            Self::User(user_buffer) => {
                let bytes = std::cmp::min(max_bytes, user_buffer.length);
                current_task
                    .read_buffer(&UserBuffer { address: user_buffer.address, length: bytes })
            }
        }
    }

    pub fn to_vec(self, current_task: &CurrentTask) -> Result<Vec<u8>, Errno> {
        match self {
            Self::Value(buffer) => Ok(buffer),
            Self::User(user_buffer) => current_task.read_buffer(&user_buffer),
        }
    }
}

// Trait used to provide `read_from_sockopt_value` for `MappingMultiArchUserRef`.
pub trait ReadFromSockOptValue {
    type Result;
    fn read_from_sockopt_value(
        current_task: &CurrentTask,
        buffer: &SockOptValue,
    ) -> Result<Self::Result, Errno>;
}

impl<T, T64, T32> ReadFromSockOptValue for MappingMultiArchUserRef<T, T64, T32>
where
    T64: FromBytes + TryInto<T>,
    T32: FromBytes + TryInto<T>,
{
    type Result = T;
    fn read_from_sockopt_value(
        current_task: &CurrentTask,
        buffer: &SockOptValue,
    ) -> Result<T, Errno> {
        match buffer {
            SockOptValue::Value(buffer) => {
                Self::read_from_prefix(current_task, &buffer).map_err(|_| errno!(EINVAL))
            }
            SockOptValue::User(user_buffer) => {
                let user_ref = Self::new_with_ref(current_task, user_buffer.clone())?;
                current_task.read_multi_arch_object(user_ref)
            }
        }
    }
}

impl Socket {
    /// Creates a new unbound socket.
    ///
    /// # Parameters
    /// - `domain`: The domain of the socket (e.g., `AF_UNIX`).
    pub fn new(
        current_task: &CurrentTask,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
        kernel_private: bool,
    ) -> Result<SocketHandle, Errno> {
        let protocol = resolve_protocol(domain, socket_type, protocol);
        // Checking access in `Socket::new()` prevents creating socket handles when not allowed,
        // while skipping the "create" permission check for accepted sockets created with
        // `Socket::new_with_ops()` and `Socket::new_with_ops_and_info()`.
        security::check_socket_create_access(
            current_task,
            domain,
            socket_type,
            protocol,
            kernel_private,
        )?;
        let ops = create_socket_ops(current_task, domain, socket_type, protocol)?;
        Ok(Self::new_with_ops_and_info(ops, domain, socket_type, protocol))
    }

    pub fn new_with_ops(ops: Box<dyn SocketOps>) -> Result<SocketHandle, Errno> {
        let (domain, socket_type, protocol) = ops.get_socket_info()?;
        Ok(Self::new_with_ops_and_info(ops, domain, socket_type, protocol))
    }

    pub fn new_with_ops_and_info(
        ops: Box<dyn SocketOps>,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> SocketHandle {
        Arc::new(Socket {
            ops,
            domain,
            socket_type,
            protocol,
            state: Default::default(),
            security: security::SocketState::default(),
        })
    }

    pub(super) fn set_fs_node(&self, node: &FsNodeHandle) {
        let mut locked_state = self.state.lock();
        assert!(locked_state.fs_node.is_none());
        locked_state.fs_node = Some(node.clone());
    }

    /// Returns the Socket that this FileHandle refers to. If this file is not a socket file,
    /// returns ENOTSOCK.
    pub fn get_from_file(file: &FileHandle) -> Result<&SocketHandle, Errno> {
        let socket_file = file.downcast_file::<SocketFile>().ok_or_else(|| errno!(ENOTSOCK))?;
        Ok(&socket_file.socket)
    }

    pub fn downcast_socket<T>(&self) -> Option<&T>
    where
        T: 'static,
    {
        let ops = &*self.ops;
        ops.as_any().downcast_ref::<T>()
    }

    pub fn getsockname(&self) -> Result<SocketAddress, Errno> {
        self.ops.getsockname(self)
    }

    pub fn getpeername(&self) -> Result<SocketAddress, Errno> {
        self.ops.getpeername(self)
    }

    pub fn setsockopt(
        &self,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        let read_timeval = || {
            let timeval = TimeValPtr::read_from_sockopt_value(current_task, &optval)?;
            let duration = duration_from_timeval(timeval)?;
            Ok(if duration == zx::MonotonicDuration::default() { None } else { Some(duration) })
        };

        security::check_socket_setsockopt_access(current_task, self, level, optname)?;
        match (level, optname) {
            (SOL_SOCKET, SO_RCVTIMEO) => self.state.lock().receive_timeout = read_timeval()?,
            (SOL_SOCKET, SO_SNDTIMEO) => self.state.lock().send_timeout = read_timeval()?,
            _ => self.ops.setsockopt(self, current_task, level, optname, optval)?,
        }
        Ok(())
    }

    pub fn getsockopt(
        &self,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        security::check_socket_getsockopt_access(current_task, self, level, optname)?;
        let value = match level {
            SOL_SOCKET => match optname {
                SO_TYPE => self.socket_type.as_raw().to_ne_bytes().to_vec(),
                SO_DOMAIN => {
                    let domain = self.domain.as_raw() as u32;
                    domain.to_ne_bytes().to_vec()
                }
                SO_PROTOCOL => self.protocol.as_raw().to_ne_bytes().to_vec(),
                SO_RCVTIMEO => {
                    let duration = self.receive_timeout().unwrap_or_default();
                    TimeValPtr::into_bytes(current_task, timeval_from_duration(duration))
                        .map_err(|_| errno!(EINVAL))?
                }
                SO_SNDTIMEO => {
                    let duration = self.send_timeout().unwrap_or_default();
                    TimeValPtr::into_bytes(current_task, timeval_from_duration(duration))
                        .map_err(|_| errno!(EINVAL))?
                }
                SO_ANDROID_DROP_REASON => {
                    track_stub!(
                        TODO("https://fxbug.dev/477273398"),
                        "Faking SO_ANDROID_DROP_REASON"
                    );
                    ANDROID_DROP_REASON_NONE.to_ne_bytes().to_vec()
                }
                _ => self.ops.getsockopt(self, current_task, level, optname, optlen)?,
            },
            _ => self.ops.getsockopt(self, current_task, level, optname, optlen)?,
        };
        Ok(value)
    }

    pub fn receive_timeout(&self) -> Option<zx::MonotonicDuration> {
        self.state.lock().receive_timeout
    }

    pub fn send_timeout(&self) -> Option<zx::MonotonicDuration> {
        self.state.lock().send_timeout
    }

    pub fn ioctl(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let res = super::netlink_ioctl::netlink_ioctl(current_task, request, arg);
        match &res {
            Err(e) if e.code == ENOTTY => {}
            _ => return res,
        }
        self.ops.ioctl(self, file, current_task, request, arg)
    }

    pub fn bind(
        &self,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        self.ops.bind(self, current_task, socket_address)
    }

    pub fn listen(&self, current_task: &CurrentTask, backlog: i32) -> Result<(), Errno> {
        security::check_socket_listen_access(current_task, self, backlog)?;
        let max_connections =
            current_task.kernel().system_limits.socket.max_connections.load(Ordering::Relaxed);
        let backlog = std::cmp::min(backlog, max_connections);
        let credentials = current_task.current_ucred();
        self.ops.listen(self, backlog, credentials)?;
        self.state.lock().bpf_state = SocketBpfState::Listen;
        Ok(())
    }

    pub fn accept(&self, current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        let new_socket = self.ops.accept(self, current_task)?;
        new_socket.state.lock().bpf_state = SocketBpfState::Established;
        Ok(new_socket)
    }

    pub fn read(
        &self,
        current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        security::check_socket_recvmsg_access(current_task, self)?;
        self.ops.read(self, current_task, data, flags)
    }

    pub fn write(
        &self,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        dest_address: &mut Option<SocketAddress>,
        ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        security::check_socket_sendmsg_access(current_task, self)?;
        self.ops.write(self, current_task, data, dest_address, ancillary_data)
    }

    pub fn wait_async(
        &self,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.ops.wait_async(self, current_task, waiter, events, handler)
    }

    pub fn query_events(&self, current_task: &CurrentTask) -> Result<FdEvents, Errno> {
        self.ops.query_events(self, current_task)
    }

    pub fn shutdown(
        &self,
        current_task: &CurrentTask,
        how: SocketShutdownFlags,
    ) -> Result<(), Errno> {
        security::check_socket_shutdown_access(current_task, self, how)?;
        self.ops.shutdown(self, how)
    }

    pub fn close(&self, current_task: &CurrentTask) {
        self.state.lock().bpf_state = SocketBpfState::Close;
        self.ops.close(current_task, self)
    }

    pub fn to_handle(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        self.ops.to_handle(self, current_task)
    }

    /// Returns the [`crate::vfs::FsNode`] unique to this `Socket`.
    // TODO: https://fxbug.dev/414583985 - Create `FsNode` at `Socket` creation and make this
    // infallible.
    pub fn fs_node(&self) -> Option<FsNodeHandle> {
        self.state.lock().fs_node.clone()
    }

    pub fn bpf_state(&self) -> SocketBpfState {
        self.state.lock().bpf_state
    }

    pub fn set_bpf_state(&self, state: SocketBpfState) {
        self.state.lock().bpf_state = state;
    }
}

impl DowncastedFile<'_, SocketFile> {
    pub fn connect(self, current_task: &CurrentTask, peer: SocketPeer) -> Result<(), Errno> {
        security::check_socket_connect_access(current_task, self, &peer)?;
        let res = self.socket.ops.connect(&self.socket, current_task, peer);
        if res.is_ok() || res == error!(EINPROGRESS) {
            self.socket.state.lock().bpf_state = SocketBpfState::Established;
        }
        res
    }
}

pub struct AcceptQueue {
    pub sockets: VecDeque<SocketHandle>,
    pub backlog: usize,
}

impl AcceptQueue {
    pub fn new(backlog: usize) -> AcceptQueue {
        AcceptQueue { sockets: VecDeque::with_capacity(backlog), backlog }
    }

    pub fn set_backlog(&mut self, backlog: usize) -> Result<(), Errno> {
        if self.sockets.len() > backlog {
            return error!(EINVAL);
        }
        self.backlog = backlog;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{map_memory, spawn_kernel_and_run};
    use crate::vfs::{UnixControlData, VecInputBuffer, VecOutputBuffer};
    use starnix_uapi::SO_PASSCRED;
    use starnix_uapi::user_address::{UserAddress, UserRef};

    #[fuchsia::test]
    async fn test_dgram_socket() {
        spawn_kernel_and_run(async |current_task| {
            let bind_address = SocketAddress::Unix(b"dgram_test".into());
            let rec_dgram = Socket::new(
                &current_task,
                SocketDomain::Unix,
                SocketType::Datagram,
                SocketProtocol::default(),
                /* kernel_private = */ false,
            )
            .expect("Failed to create socket.");
            let passcred: u32 = 1;
            let opt_size = std::mem::size_of::<u32>();
            let user_address = map_memory(&current_task, UserAddress::default(), opt_size as u64);
            let opt_ref = UserRef::<u32>::new(user_address);
            current_task.write_object(opt_ref, &passcred).unwrap();
            let opt_buf = UserBuffer { address: user_address, length: opt_size };
            rec_dgram.setsockopt(&current_task, SOL_SOCKET, SO_PASSCRED, opt_buf.into()).unwrap();

            rec_dgram.bind(&current_task, bind_address).expect("failed to bind datagram socket");

            let xfer_value: u64 = 1234567819;
            let xfer_bytes = xfer_value.to_ne_bytes();

            let send = Socket::new(
                &current_task,
                SocketDomain::Unix,
                SocketType::Datagram,
                SocketProtocol::default(),
                /* kernel_private = */ false,
            )
            .expect("Failed to connect socket.");
            send.ops.connect(&send, &current_task, SocketPeer::Handle(rec_dgram.clone())).unwrap();
            let mut source_iter = VecInputBuffer::new(&xfer_bytes);
            send.write(&current_task, &mut source_iter, &mut None, &mut vec![]).unwrap();
            assert_eq!(source_iter.available(), 0);
            // Previously, this would cause the test to fail,
            // because rec_dgram was shut down.
            send.close(&current_task);

            let mut rec_buffer = VecOutputBuffer::new(8);
            let read_info = rec_dgram
                .read(&current_task, &mut rec_buffer, SocketMessageFlags::empty())
                .unwrap();
            assert_eq!(read_info.bytes_read, xfer_bytes.len());
            assert_eq!(rec_buffer.data(), xfer_bytes);
            assert_eq!(1, read_info.ancillary_data.len());
            assert_eq!(
                read_info.ancillary_data[0],
                AncillaryData::Unix(UnixControlData::Credentials(uapi::ucred {
                    pid: current_task.get_pid(),
                    uid: 0,
                    gid: 0
                }))
            );

            rec_dgram.close(&current_task);
        })
        .await;
    }
}
