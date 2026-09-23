// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security::{self, AuditLogger, AuditMessage, AuditRequest};
use crate::vfs::socket::{SockOptValue, SocketDomain};
use futures::channel::mpsc::{
    UnboundedReceiver, UnboundedSender, {self},
};
use linux_uapi::{AUDIT_GET, NETLINK_GET_STRICT_CHK, audit_status};
use netlink::messaging::{
    AccessControl, MessageWithPermission, NetlinkContext, NetlinkMessageWithCreds, Permission,
    Sender, UnparsedNetlinkMessage,
};
use netlink::multicast_groups::{
    InvalidLegacyGroupsError, InvalidModernGroupError, LegacyGroups, ModernGroup,
    NoMappingFromModernToLegacyGroupError, SingleLegacyGroup,
};
use netlink::protocol_family::NetlinkClient;
use netlink::protocol_family::route::NetlinkRouteClient;
use netlink::protocol_family::sock_diag::NetlinkSockDiagClient;
use netlink::{NETLINK_LOG_TAG, NewClientError};
use netlink_packet_core::{
    ErrorMessage, NETLINK_HEADER_LEN, NLMSG_ERROR, NetlinkBuffer, NetlinkDeserializable,
    NetlinkHeader, NetlinkMessage, NetlinkPayload, NetlinkSerializable,
};
use netlink_packet_generic::message::EmptyDeserializeOptions as EmptyDeserializeGenlOptions;
use netlink_packet_route::{RouteNetlinkMessage, RouteNetlinkMessageParseMode};
use netlink_packet_sock_diag::SockDiagRequest;
use netlink_packet_sock_diag::message::EmptyDeserializeOptions as EmptyDeserializeSockDiagOptions;
use netlink_packet_utils::{DecodeError, Emitable as _};
use starnix_sync::{
    AuditNetlinkClientAuditResponseLock, LockDepGuard, LockDepMutex, NetlinkSocketInnerLock,
    UEventNetlinkSocketDeviceListenerKeyLock,
};
use std::io::Write;
use std::marker::PhantomData;
use std::num::{NonZeroI32, NonZeroU32};
use std::sync::Arc;
use zerocopy::{FromBytes, IntoBytes};

use crate::device::kobject::{Device, UEventAction, UEventContext, flatten_uevent_properties};
use crate::device::{DeviceListener, DeviceListenerKey};
use crate::task::{CurrentTask, EventHandler, Kernel, WaitCanceler, WaitQueue, Waiter};
use crate::vfs::buffers::{
    AncillaryData, InputBuffer, Message, MessageQueue, MessageReadInfo, OutputBuffer,
    UnixControlData, VecInputBuffer,
};
use crate::vfs::socket::{
    GenericMessage, GenericNetlinkClientHandle, Socket, SocketAddress, SocketHandle,
    SocketMessageFlags, SocketOps, SocketPeer, SocketShutdownFlags, SocketType,
};
use starnix_logging::{log_debug, log_error, log_warn, track_stub};
use starnix_uapi::auth::{CAP_AUDIT_CONTROL, CAP_AUDIT_WRITE, CAP_NET_ADMIN, Credentials};
use starnix_uapi::errors::Errno;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    AF_NETLINK, NETLINK_ADD_MEMBERSHIP, NETLINK_AUDIT, NETLINK_CONNECTOR, NETLINK_CRYPTO,
    NETLINK_DNRTMSG, NETLINK_DROP_MEMBERSHIP, NETLINK_ECRYPTFS, NETLINK_FIB_LOOKUP,
    NETLINK_FIREWALL, NETLINK_GENERIC, NETLINK_IP6_FW, NETLINK_ISCSI, NETLINK_KOBJECT_UEVENT,
    NETLINK_NETFILTER, NETLINK_NFLOG, NETLINK_RDMA, NETLINK_ROUTE, NETLINK_SCSITRANSPORT,
    NETLINK_SELINUX, NETLINK_SMC, NETLINK_SOCK_DIAG, NETLINK_USERSOCK, NETLINK_XFRM, NLM_F_MULTI,
    NLMSG_DONE, SO_PASSCRED, SO_PROTOCOL, SO_RCVBUF, SO_RCVBUFFORCE, SO_SNDBUF, SO_SNDBUFFORCE,
    SO_TIMESTAMP, SOL_SOCKET, errno, error, nlmsghdr, sockaddr_nl, socklen_t, ucred,
};

// From netlink/socket.go in gVisor.
pub const SOCKET_MIN_SIZE: usize = 4 << 10;
pub const SOCKET_DEFAULT_SIZE: usize = 16 * 1024;
pub const SOCKET_MAX_SIZE: usize = 4 << 20;

// From linux/socket.go in gVisor.
const SOL_NETLINK: u32 = 270;

pub fn new_netlink_socket(
    kernel: &Arc<Kernel>,
    socket_type: SocketType,
    family: NetlinkFamily,
) -> Result<Box<dyn SocketOps>, Errno> {
    log_debug!(tag = NETLINK_LOG_TAG; "Creating {:?} Netlink Socket", family);
    if socket_type != SocketType::Datagram && socket_type != SocketType::Raw {
        return error!(ESOCKTNOSUPPORT);
    }

    let ops: Box<dyn SocketOps> = match family {
        NetlinkFamily::KobjectUevent => Box::new(UEventNetlinkSocket::default()),
        NetlinkFamily::Route => Box::new(new_route_socket(kernel)?),
        NetlinkFamily::Generic => Box::new(GenericNetlinkSocket::new(kernel)?),
        NetlinkFamily::SockDiag => Box::new(new_sock_diag_socket(kernel)?),
        NetlinkFamily::Audit => Box::new(AuditNetlinkSocket::new(kernel)?),
        NetlinkFamily::Nflog => {
            let inner = Arc::new(NetlinkSocketInner::new(NetlinkFamily::Nflog).into());
            Box::new(NflogNetlinkSocket::new(inner))
        }
        NetlinkFamily::Usersock
        | NetlinkFamily::Firewall
        | NetlinkFamily::Xfrm
        | NetlinkFamily::Selinux
        | NetlinkFamily::Iscsi
        | NetlinkFamily::FibLookup
        | NetlinkFamily::Connector
        | NetlinkFamily::Netfilter
        | NetlinkFamily::Ip6Fw
        | NetlinkFamily::Dnrtmsg
        | NetlinkFamily::Scsitransport
        | NetlinkFamily::Ecryptfs
        | NetlinkFamily::Rdma
        | NetlinkFamily::Crypto
        | NetlinkFamily::Smc => Box::new(StubbedNetlinkSocket::new(family)),
        NetlinkFamily::Invalid => return error!(EINVAL),
    };
    Ok(ops)
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct NetlinkAddress {
    pid: u32,
    groups: u32,
}

impl NetlinkAddress {
    pub fn new(pid: u32, groups: u32) -> Self {
        NetlinkAddress { pid, groups }
    }

    pub fn set_pid_if_zero(&mut self, pid: i32) {
        if self.pid == 0 {
            self.pid = pid as u32;
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        sockaddr_nl { nl_family: AF_NETLINK, nl_pid: self.pid, nl_pad: 0, nl_groups: self.groups }
            .as_bytes()
            .to_vec()
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub enum NetlinkFamily {
    Invalid,
    Route,
    Usersock,
    Firewall,
    SockDiag,
    Nflog,
    Xfrm,
    Selinux,
    Iscsi,
    Audit,
    FibLookup,
    Connector,
    Netfilter,
    Ip6Fw,
    Dnrtmsg,
    KobjectUevent,
    Generic,
    Scsitransport,
    Ecryptfs,
    Rdma,
    Crypto,
    Smc,
}

impl NetlinkFamily {
    pub fn from_raw(family: u32) -> Self {
        match family {
            NETLINK_ROUTE => NetlinkFamily::Route,
            NETLINK_USERSOCK => NetlinkFamily::Usersock,
            NETLINK_FIREWALL => NetlinkFamily::Firewall,
            NETLINK_SOCK_DIAG => NetlinkFamily::SockDiag,
            NETLINK_NFLOG => NetlinkFamily::Nflog,
            NETLINK_XFRM => NetlinkFamily::Xfrm,
            NETLINK_SELINUX => NetlinkFamily::Selinux,
            NETLINK_ISCSI => NetlinkFamily::Iscsi,
            NETLINK_AUDIT => NetlinkFamily::Audit,
            NETLINK_FIB_LOOKUP => NetlinkFamily::FibLookup,
            NETLINK_CONNECTOR => NetlinkFamily::Connector,
            NETLINK_NETFILTER => NetlinkFamily::Netfilter,
            NETLINK_IP6_FW => NetlinkFamily::Ip6Fw,
            NETLINK_DNRTMSG => NetlinkFamily::Dnrtmsg,
            NETLINK_KOBJECT_UEVENT => NetlinkFamily::KobjectUevent,
            NETLINK_GENERIC => NetlinkFamily::Generic,
            NETLINK_SCSITRANSPORT => NetlinkFamily::Scsitransport,
            NETLINK_ECRYPTFS => NetlinkFamily::Ecryptfs,
            NETLINK_RDMA => NetlinkFamily::Rdma,
            NETLINK_CRYPTO => NetlinkFamily::Crypto,
            NETLINK_SMC => NetlinkFamily::Smc,
            _ => NetlinkFamily::Invalid,
        }
    }

    pub fn as_raw(&self) -> u32 {
        match self {
            NetlinkFamily::Route => NETLINK_ROUTE,
            NetlinkFamily::KobjectUevent => NETLINK_KOBJECT_UEVENT,
            NetlinkFamily::Audit => NETLINK_AUDIT,
            _ => 0,
        }
    }
}

struct NetlinkSocketInner {
    /// The specific type of netlink socket.
    family: NetlinkFamily,

    /// The [`MessageQueue`] that contains messages from netlink to the client.
    receive_buffer: MessageQueue,

    /// The socket's send buffer size. Note, This value is only used
    /// to serve getsockopt calls for `SO_SNDBUF`. It does not yet enforce a
    /// limit on the number of messages netlink will buffer from the client.
    /// TODO(https://fxbug.dev/285880057): Limit the size of the send buffer.
    send_buf_size: usize,

    /// This queue will be notified on reads, writes, disconnects etc.
    waiters: WaitQueue,

    /// The address of this socket.
    address: Option<NetlinkAddress>,

    /// See SO_PASSCRED.
    pub passcred: bool,

    /// See SO_TIMESTAMP.
    pub timestamp: bool,

    /// See NETLINK_GET_STRICT_CHK.
    pub strict_chk: bool,
}

impl NetlinkSocketInner {
    fn new(family: NetlinkFamily) -> Self {
        Self {
            family,
            receive_buffer: MessageQueue::new(SOCKET_DEFAULT_SIZE),
            send_buf_size: SOCKET_DEFAULT_SIZE,
            waiters: WaitQueue::default(),
            address: None,
            passcred: false,
            timestamp: false,
            strict_chk: false,
        }
    }

    fn bind(
        &mut self,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        if self.address.is_some() {
            return error!(EINVAL);
        }

        let netlink_address = match socket_address {
            SocketAddress::Netlink(mut netlink_address) => {
                // TODO: Support distinct IDs for processes with multiple netlink sockets.
                netlink_address.set_pid_if_zero(current_task.get_pid());
                netlink_address
            }
            _ => return error!(EINVAL),
        };

        self.address = Some(netlink_address);
        Ok(())
    }

    fn connect(&mut self, current_task: &CurrentTask, peer: SocketPeer) -> Result<(), Errno> {
        let address = match peer {
            SocketPeer::Address(address) => address,
            _ => return error!(EINVAL),
        };
        // Connect is equivalent to bind, but error are ignored.
        let _ = self.bind(current_task, address);
        Ok(())
    }

    fn read_message(&mut self) -> Option<Message> {
        let message = self.receive_buffer.read_message();
        if message.is_some() {
            self.waiters.notify_fd_events(FdEvents::POLLOUT);
        }
        message
    }

    fn read_datagram(
        &mut self,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        let (mut info, has_message) = if flags.contains(SocketMessageFlags::PEEK) {
            self.receive_buffer.peek_datagram(data)
        } else {
            self.receive_buffer.read_datagram(data)
        }?;
        if !has_message {
            return error!(EAGAIN);
        }

        if self.passcred {
            track_stub!(TODO("https://fxbug.dev/297373991"), "SCM_CREDENTIALS/SO_PASSCRED");
            info.ancillary_data.push(AncillaryData::Unix(UnixControlData::unknown_creds()));
        }

        Ok(info)
    }

    fn write_to_queue(
        &mut self,
        data: &mut dyn InputBuffer,
        address: Option<NetlinkAddress>,
        ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let socket_address = match address {
            Some(addr) => Some(SocketAddress::Netlink(addr)),
            None => self.address.as_ref().map(|addr| SocketAddress::Netlink(addr.clone())),
        };
        let bytes_written =
            self.receive_buffer.write_datagram(data, socket_address, ancillary_data)?;
        if bytes_written > 0 {
            self.waiters.notify_fd_events(FdEvents::POLLIN);
        }
        Ok(bytes_written)
    }

    fn wait_async(
        &mut self,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.waiters.wait_async_fd_events(waiter, events, handler)
    }

    fn query_events(&self) -> FdEvents {
        self.receive_buffer.query_events()
    }

    fn getsockname(&self) -> Result<SocketAddress, Errno> {
        match &self.address {
            Some(addr) => Ok(SocketAddress::Netlink(addr.clone())),
            _ => Ok(SocketAddress::default_for_domain(SocketDomain::Netlink)),
        }
    }

    fn getpeername(&self) -> Result<SocketAddress, Errno> {
        match &self.address {
            Some(addr) => Ok(SocketAddress::Netlink(addr.clone())),
            _ => Ok(SocketAddress::default_for_domain(SocketDomain::Netlink)),
        }
    }

    fn getsockopt(&self, level: u32, optname: u32) -> Result<Vec<u8>, Errno> {
        let opt_value = match level {
            SOL_SOCKET => match optname {
                SO_PASSCRED => (self.passcred as u32).as_bytes().to_vec(),
                SO_TIMESTAMP => (self.timestamp as u32).as_bytes().to_vec(),
                SO_SNDBUF => (self.send_buf_size as socklen_t).to_ne_bytes().to_vec(),
                SO_RCVBUF => (self.receive_buffer.capacity() as socklen_t).to_ne_bytes().to_vec(),
                SO_SNDBUFFORCE => (self.send_buf_size as socklen_t).to_ne_bytes().to_vec(),
                SO_RCVBUFFORCE => {
                    (self.receive_buffer.capacity() as socklen_t).to_ne_bytes().to_vec()
                }
                SO_PROTOCOL => self.family.as_raw().as_bytes().to_vec(),
                _ => return error!(ENOSYS),
            },
            SOL_NETLINK => match optname {
                NETLINK_GET_STRICT_CHK => (self.strict_chk as u32).as_bytes().to_vec(),
                _ => return error!(ENOSYS),
            },
            _ => vec![],
        };

        Ok(opt_value)
    }

    fn setsockopt(
        &mut self,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        match level {
            SOL_SOCKET => match optname {
                SO_SNDBUF => {
                    let requested_capacity: socklen_t = optval.read(current_task)?;
                    // SO_SNDBUF doubles the requested capacity to leave space for bookkeeping.
                    // See https://man7.org/linux/man-pages/man7/socket.7.html
                    let capacity = usize::try_from(requested_capacity * 2).unwrap_or(usize::MAX);
                    // TODO(https://fxbug.dev/322907334): Clamp to `wmem_max`.
                    let capacity = capacity.clamp(SOCKET_MIN_SIZE, SOCKET_MAX_SIZE);
                    self.send_buf_size = capacity;
                }
                SO_SNDBUFFORCE => {
                    security::check_task_capable(current_task, CAP_NET_ADMIN)?;
                    let requested_capacity: socklen_t = optval.read(current_task)?;
                    // SO_SNDBUFFORE doubles the requested capacity to leave space for bookkeeping.
                    // See https://man7.org/linux/man-pages/man7/socket.7.html
                    let capacity = usize::try_from(requested_capacity * 2).unwrap_or(usize::MAX);
                    self.send_buf_size = capacity;
                }
                SO_RCVBUF => {
                    let requested_capacity: socklen_t = optval.read(current_task)?;
                    // SO_RCVBUF doubles the requested capacity to leave space for bookkeeping.
                    // See https://man7.org/linux/man-pages/man7/socket.7.html
                    let capacity = usize::try_from(requested_capacity * 2).unwrap_or(usize::MAX);
                    // TODO(https://fxbug.dev/322906968): Clamp to `rmem_max`.
                    let capacity = capacity.clamp(SOCKET_MIN_SIZE, SOCKET_MAX_SIZE);
                    self.receive_buffer.set_capacity(capacity)?;
                }
                SO_RCVBUFFORCE => {
                    security::check_task_capable(current_task, CAP_NET_ADMIN)?;
                    let requested_capacity: socklen_t = optval.read(current_task)?;
                    // SO_RCVBUFFORE doubles the requested capacity to leave space for bookkeeping.
                    // See https://man7.org/linux/man-pages/man7/socket.7.html
                    let capacity = usize::try_from(requested_capacity * 2).unwrap_or(usize::MAX);
                    self.receive_buffer.set_capacity(capacity)?;
                }
                SO_PASSCRED => {
                    let passcred: u32 = optval.read(current_task)?;
                    self.passcred = passcred != 0;
                }
                SO_TIMESTAMP => {
                    let timestamp: u32 = optval.read(current_task)?;
                    self.timestamp = timestamp != 0;
                }
                _ => return error!(ENOSYS),
            },
            SOL_NETLINK => match optname {
                NETLINK_GET_STRICT_CHK => {
                    let strict_chk: u32 = optval.read(current_task)?;
                    self.strict_chk = strict_chk != 0;
                }
                _ => return error!(ENOSYS),
            },
            _ => return error!(ENOSYS),
        }

        Ok(())
    }
}

struct NflogListener {
    inner: std::sync::Weak<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>,
}

static NFLOG_LISTENERS: std::sync::LazyLock<std::sync::Mutex<Vec<NflogListener>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

struct NflogNetlinkSocket {
    inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>,
}

impl NflogNetlinkSocket {
    fn new(inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>) -> Self {
        NFLOG_LISTENERS.lock().unwrap().push(NflogListener { inner: Arc::downgrade(&inner) });
        Self { inner }
    }

    fn lock(&self) -> LockDepGuard<'_, NetlinkSocketInner> {
        self.inner.lock()
    }
}

impl Drop for NflogNetlinkSocket {
    fn drop(&mut self) {
        let mut listeners = NFLOG_LISTENERS.lock().unwrap();
        listeners.retain(|l| {
            if let Some(arc) = l.inner.upgrade() { !Arc::ptr_eq(&arc, &self.inner) } else { false }
        });
    }
}

impl SocketOps for NflogNetlinkSocket {
    fn connect(
        &self,
        _socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        self.lock().connect(current_task, peer)
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        self.lock().bind(current_task, socket_address)
    }

    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        self.lock().read_datagram(data, flags)
    }

    fn write(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _data: &mut dyn InputBuffer,
        _dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        error!(EOPNOTSUPP)
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.lock().wait_async(waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(self.lock().query_events() & FdEvents::POLLIN)
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        Ok(())
    }

    fn close(&self, _current_task: &CurrentTask, _socket: &Socket) {}

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getsockname()
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getpeername()
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        self.lock().getsockopt(level, optname)
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        self.lock().setsockopt(current_task, level, optname, optval)
    }
}

fn write_attr(buf: &mut Vec<u8>, attr_type: u16, data: &[u8]) {
    let len = 4 + data.len();
    buf.extend_from_slice(&(len as u16).to_ne_bytes());
    buf.extend_from_slice(&attr_type.to_ne_bytes());
    buf.extend_from_slice(data);
    let pad = (4 - (data.len() % 4)) % 4;
    for _ in 0..pad {
        buf.push(0);
    }
}

pub fn send_fake_nflog_message(uid: u32) {
    const NFNL_SUBSYS_ULOG: u16 = 4;
    const NFULNL_MSG_PACKET: u8 = 0;
    const LOCAL_NFLOG_PACKET: u16 = (NFNL_SUBSYS_ULOG << 8) | (NFULNL_MSG_PACKET as u16);
    const NFULA_PAYLOAD: u16 = 9;
    const NFULA_UID: u16 = 11;

    let mut msg_body = vec![0u8; 4]; // 4 bytes of nfgenmsg header

    // Write UID attribute
    let uid_bytes = uid.to_be_bytes();
    write_attr(&mut msg_body, NFULA_UID, &uid_bytes);

    // Write PAYLOAD attribute (placeholder IPv4 header)
    let mut ip_hdr = vec![0u8; 20];
    ip_hdr[0] = 0x45; // Version 4, IHL 5
    ip_hdr[12..16].copy_from_slice(&[127, 0, 0, 1]);
    ip_hdr[16..20].copy_from_slice(&[8, 8, 8, 8]);
    write_attr(&mut msg_body, NFULA_PAYLOAD, &ip_hdr);

    // Construct nlmsghdr
    let total_len = 16 + msg_body.len();
    let mut packet = vec![];
    packet.extend_from_slice(&(total_len as u32).to_ne_bytes());
    packet.extend_from_slice(&LOCAL_NFLOG_PACKET.to_ne_bytes());
    packet.extend_from_slice(&0u16.to_ne_bytes()); // flags
    packet.extend_from_slice(&0u32.to_ne_bytes()); // seq
    packet.extend_from_slice(&0u32.to_ne_bytes()); // pid
    packet.extend_from_slice(&msg_body);

    // Broadcast to all listeners
    let listeners = NFLOG_LISTENERS.lock().unwrap();
    let ancillary_data = AncillaryData::Unix(UnixControlData::Credentials(Default::default()));
    let mut ancillary_data = vec![ancillary_data];

    for listener in listeners.iter() {
        if let Some(socket) = listener.inner.upgrade() {
            let _ = socket.lock().write_to_queue(
                &mut VecInputBuffer::new(&packet),
                Some(NetlinkAddress { pid: 0, groups: 1 }),
                &mut ancillary_data,
            );
        }
    }
}

/// A fake Netlink socket that loops messages back to the client.
///
/// Used as a placeholder implementation for protocol families that lack a real
/// implementation.
struct StubbedNetlinkSocket {
    inner: LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>,
}

impl StubbedNetlinkSocket {
    pub fn new(family: NetlinkFamily) -> Self {
        track_stub!(
            TODO("https://fxbug.dev/278565021"),
            format!("Creating StubbedNetlinkSocket: {:?}", family).as_str()
        );
        StubbedNetlinkSocket { inner: NetlinkSocketInner::new(family).into() }
    }

    /// Locks and returns the inner state of the Socket.
    fn lock(&self) -> LockDepGuard<'_, NetlinkSocketInner> {
        self.inner.lock()
    }
}

impl SocketOps for StubbedNetlinkSocket {
    fn connect(
        &self,
        _socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        self.lock().connect(current_task, peer)
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        self.lock().bind(current_task, socket_address)
    }

    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        _flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        let msg = self.lock().read_message();
        match msg {
            Some(message) => {
                // Mark the message as complete and return it.
                let (mut nl_msg, _) =
                    nlmsghdr::read_from_prefix(&message.data).map_err(|_| errno!(EINVAL))?;
                nl_msg.nlmsg_type = NLMSG_DONE as u16;
                nl_msg.nlmsg_flags &= NLM_F_MULTI as u16;
                let msg_bytes = nl_msg.as_bytes();
                let bytes_read = data.write(msg_bytes)?;

                let info = MessageReadInfo {
                    bytes_read,
                    message_length: msg_bytes.len(),
                    address: Some(SocketAddress::Netlink(NetlinkAddress::default())),
                    ancillary_data: vec![],
                };
                Ok(info)
            }
            None => Ok(MessageReadInfo::default()),
        }
    }

    fn write(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        dest_address: &mut Option<SocketAddress>,
        ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let bytes = data.peek_all()?;
        if let Ok((nl_msg, _)) = nlmsghdr::read_from_prefix(&bytes) {
            security::check_netlink_send_access(current_task, socket, nl_msg.nlmsg_type)?;
        }

        let mut local_address = self.lock().address.clone();

        let destination = match dest_address {
            Some(SocketAddress::Netlink(addr)) => addr,
            _ => match &mut local_address {
                Some(addr) => addr,
                _ => return Ok(data.drain()),
            },
        };

        if destination.groups != 0 {
            track_stub!(TODO("https://fxbug.dev/322874956"), "StubbedNetlinkSockets multicasting");
            return Ok(data.drain());
        }

        self.lock().write_to_queue(data, Some(NetlinkAddress::default()), ancillary_data)
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.lock().wait_async(waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(self.lock().query_events() & FdEvents::POLLIN)
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/322875507"), "StubbedNetlinkSocket::shutdown");
        Ok(())
    }

    fn close(&self, _current_task: &CurrentTask, _socket: &Socket) {}

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getsockname()
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getpeername()
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        self.lock().getsockopt(level, optname)
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        self.lock().setsockopt(current_task, level, optname, optval)
    }
}

/// Socket implementation for the NETLINK_KOBJECT_UEVENT family of netlink sockets.
struct UEventNetlinkSocket {
    inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>,
    device_listener_key:
        LockDepMutex<Option<DeviceListenerKey>, UEventNetlinkSocketDeviceListenerKeyLock>,
}

impl Default for UEventNetlinkSocket {
    #[allow(clippy::let_and_return)]
    fn default() -> Self {
        let result = Self {
            inner: Arc::new(NetlinkSocketInner::new(NetlinkFamily::KobjectUevent).into()),
            device_listener_key: Default::default(),
        };
        #[cfg(any(test, debug_assertions))]
        {
            let _l1 = result.device_listener_key.lock();
            let _l2 = result.lock();
        }
        result
    }
}

impl UEventNetlinkSocket {
    /// Locks and returns the inner state of the Socket.
    fn lock(&self) -> LockDepGuard<'_, NetlinkSocketInner> {
        self.inner.lock()
    }

    fn register_listener(
        &self,
        current_task: &CurrentTask,
        state: LockDepGuard<'_, NetlinkSocketInner>,
    ) {
        if state.address.is_none() {
            return;
        }
        std::mem::drop(state);
        let mut key_state = self.device_listener_key.lock();
        if key_state.is_none() {
            *key_state =
                Some(current_task.kernel().device_registry.register_listener(self.inner.clone()));
        }
    }
}

impl SocketOps for UEventNetlinkSocket {
    fn connect(
        &self,
        _socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        let mut state = self.lock();
        state.connect(current_task, peer)?;
        self.register_listener(current_task, state);
        Ok(())
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        let mut state = self.lock();
        state.bind(current_task, socket_address)?;
        self.register_listener(current_task, state);
        Ok(())
    }

    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        self.lock().read_datagram(data, flags)
    }

    fn write(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _data: &mut dyn InputBuffer,
        _dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        error!(EOPNOTSUPP)
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.lock().wait_async(waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(self.lock().query_events() & FdEvents::POLLIN)
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/322875507"), "UEventNetlinkSocket::shutdown");
        Ok(())
    }

    fn close(&self, current_task: &CurrentTask, _socket: &Socket) {
        let id = self.device_listener_key.lock().take();
        if let Some(id) = id {
            current_task.kernel().device_registry.unregister_listener(&id);
        }
    }

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getsockname()
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getpeername()
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        self.lock().getsockopt(level, optname)
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        self.lock().setsockopt(current_task, level, optname, optval)
    }
}

impl DeviceListener for Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>> {
    fn on_device_event(&self, action: UEventAction, device: Device, context: UEventContext) {
        let path = device.path_from_depth(0);

        let mut props = device.get_uevent_properties_list();

        // Prepend ACTION and SEQNUM to maintain existing order
        props.insert(0, (b"ACTION".into(), action.to_string().into()));
        props.insert(1, (b"SEQNUM".into(), context.seqnum.to_string().into()));

        let flattened = flatten_uevent_properties(props, '\0');

        let mut message = vec![];
        write!(&mut message, "{action}@/{path}\0", action = action, path = path).unwrap();
        message.extend_from_slice(flattened.as_ref());

        let ancillary_data = AncillaryData::Unix(UnixControlData::Credentials(Default::default()));
        let mut ancillary_data = vec![ancillary_data];
        // Ignore write errors
        let _ = self.lock().write_to_queue(
            &mut VecInputBuffer::new(&message),
            Some(NetlinkAddress { pid: 0, groups: 1 }),
            &mut ancillary_data,
        );
    }
}

/// Type for sending messages from [`netlink::Netlink`] to an individual socket.
#[derive(Clone)]
pub struct NetlinkToClientSender<M> {
    /// The inner socket implementation, which holds a message queue.
    inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>,

    /// `PhantomData<fn(M) -> M>` is used instead of `PhantomData<M>` in order
    /// to ensure that the type is invariant over `M` and that it implements
    /// `Sync` even if `M` is not `Sync`.
    _message_type: PhantomData<fn(M) -> M>,
}

impl<M> NetlinkToClientSender<M> {
    fn new(inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>) -> Self {
        NetlinkToClientSender { _message_type: Default::default(), inner }
    }
}

impl<M: Clone + NetlinkSerializable + Send> Sender<M> for NetlinkToClientSender<M> {
    fn send(&mut self, message: NetlinkMessage<M>, group: Option<ModernGroup>) {
        // Serialize the message
        let mut buf = vec![0; message.buffer_len()];
        message.emit(&mut buf);
        let mut buf: VecInputBuffer = buf.into();
        // Write the message into the inner socket buffer.
        let NetlinkToClientSender { _message_type: _, inner } = self;
        let mut guard = inner.lock();

        // To avoid dropping messages when the receive buffer is
        // full, grow the buffer on behalf of the client.
        // This is a stop gap measure to avoid dropping messages
        // when netlink produces a large response to a
        // NLM_F_DUMP request.
        //
        // TODO(https://fxbug.dev/459883760): The memory
        // implications of this may be problematic. It should be
        // replaced with a proper mechanism to handle a backlog
        // of NLM_F_DUMP responses.
        let available = guard.receive_buffer.available_capacity();
        let required = buf.available();
        if available < required {
            let delta = required - available;
            let current_capacity = guard.receive_buffer.capacity();
            let new_capacity = (current_capacity + delta).min(SOCKET_MAX_SIZE);
            match guard.receive_buffer.set_capacity(new_capacity) {
                Ok(()) => {}
                Err(e) => {
                    log_error!(
                        tag = NETLINK_LOG_TAG;
                        "Failed to increase receive buffer size: {:?}",
                        e
                    );
                }
            }
        }

        let _bytes_written: usize = guard
            .write_to_queue(
                &mut buf,
                Some(NetlinkAddress {
                    // All messages come from the "kernel" which has PID of 0.
                    pid: 0,
                    // If this is a multicast message, set the group the multicast
                    // message is from.
                    groups: group
                        .map(SingleLegacyGroup::try_from)
                        .and_then(Result::<_, NoMappingFromModernToLegacyGroupError>::ok)
                        .map_or(0, |g| g.inner()),
                }),
                &mut Vec::new(),
            )
            .unwrap_or_else(|e| {
                log_error!(
                    tag = NETLINK_LOG_TAG;
                    "Failed to write message into buffer for socket. Errno: {:?}",
                    e
                );
                0
            });
    }
}

#[derive(Clone)]
pub struct NetlinkAccessControl<'a> {
    current_task: &'a CurrentTask,
}

impl<'a> NetlinkAccessControl<'a> {
    pub fn new(current_task: &'a CurrentTask) -> Self {
        Self { current_task }
    }
}

impl<'a> AccessControl<Arc<Credentials>> for NetlinkAccessControl<'a> {
    fn grant_assess(
        &self,
        creds: &Arc<Credentials>,
        permission: Permission,
    ) -> Result<(), netlink::Errno> {
        let need_cap_net_admin = match permission {
            Permission::NetlinkRouteRead => false,
            Permission::NetlinkRouteWrite => true,
            Permission::NetlinkSockDiagRead => false,
            Permission::NetlinkSockDiagDestroy => true,
        };
        if !need_cap_net_admin {
            return Ok(());
        }

        self.current_task.override_creds(creds.clone(), || {
            security::check_task_capable(self.current_task, CAP_NET_ADMIN).map_err(|error| {
                netlink::Errno::new(error.code.error_code() as i32)
                    .expect("Errno::error_code() is expected to be in range [1..max_i32]")
            })
        })
    }
}
pub struct NetlinkContextImpl;

impl NetlinkContext for NetlinkContextImpl {
    type Creds = Arc<Credentials>;
    type Sender<M: Clone + NetlinkSerializable + Send> = NetlinkToClientSender<M>;
    type Receiver<
        M: Send + MessageWithPermission + NetlinkDeserializable<Error: Into<DecodeError>>,
    > = UnboundedReceiver<NetlinkMessageWithCreds<UnparsedNetlinkMessage<Vec<u8>, M>, Self::Creds>>;
    type AccessControl<'a> = NetlinkAccessControl<'a>;
}

fn new_route_socket(kernel: &Arc<Kernel>) -> Result<NetlinkSocket<NetlinkRouteClient>, Errno> {
    let inner = Arc::new(LockDepMutex::new(NetlinkSocketInner::new(NetlinkFamily::Route)));
    let (message_sender, message_receiver) = mpsc::unbounded();
    let client = match kernel
        .network_netlink()
        .new_route_client(NetlinkToClientSender::new(inner.clone()), message_receiver)
    {
        Ok(client) => client,
        Err(NewClientError::Disconnected) => {
            log_error!(
                tag = NETLINK_LOG_TAG;
                "Netlink async worker is unexpectedly disconnected"
            );
            return error!(EPIPE);
        }
    };
    Ok(NetlinkSocket { inner, client, message_sender })
}

fn new_sock_diag_socket(
    kernel: &Arc<Kernel>,
) -> Result<NetlinkSocket<NetlinkSockDiagClient>, Errno> {
    let inner = Arc::new(LockDepMutex::new(NetlinkSocketInner::new(NetlinkFamily::SockDiag)));
    let (message_sender, message_receiver) = mpsc::unbounded();
    let client = match kernel
        .network_netlink()
        .new_sock_diag_client(NetlinkToClientSender::new(inner.clone()), message_receiver)
    {
        Ok(client) => client,
        Err(NewClientError::Disconnected) => {
            log_error!(
                tag = NETLINK_LOG_TAG;
                "Netlink async worker is unexpectedly disconnected"
            );
            return error!(EPIPE);
        }
    };
    Ok(NetlinkSocket { inner, client, message_sender })
}

/// An abstraction over common networking-specific netlink sockets.
struct NetlinkSocket<C: NetlinkClient> {
    /// The inner Netlink socket implementation
    inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>,
    /// The implementation of a client (socket connection) to a netlink protocol
    /// family.
    client: C,
    /// The sender of messages from this socket to Netlink.
    // TODO(https://issuetracker.google.com/285880057): Bound the capacity of
    // the "send buffer".
    message_sender: UnboundedSender<
        NetlinkMessageWithCreds<UnparsedNetlinkMessage<Vec<u8>, C::Request>, Arc<Credentials>>,
    >,
}

/// A type that provides Netlink message deserialization options.
trait DeserializeOptionsProvider {
    /// The type of the message to deserialize.
    type Message: NetlinkDeserializable;
    /// The options to use when deserializing a `Message`.
    fn options(&self) -> <Self::Message as NetlinkDeserializable>::DeserializeOptions;
}

impl DeserializeOptionsProvider for NetlinkSocket<NetlinkRouteClient> {
    type Message = RouteNetlinkMessage;
    fn options(&self) -> RouteNetlinkMessageParseMode {
        let strict = self.inner.lock().strict_chk;
        if strict {
            RouteNetlinkMessageParseMode::Strict
        } else {
            RouteNetlinkMessageParseMode::Relaxed
        }
    }
}

impl DeserializeOptionsProvider for NetlinkSocket<NetlinkSockDiagClient> {
    type Message = SockDiagRequest;
    fn options(&self) -> EmptyDeserializeSockDiagOptions {
        EmptyDeserializeSockDiagOptions
    }
}

impl<C: NetlinkClient + 'static> SocketOps for NetlinkSocket<C>
where
    Self: DeserializeOptionsProvider<Message = C::Request>,
{
    fn connect(
        &self,
        _socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        let NetlinkSocket { inner, client: _, message_sender: _ } = self;
        inner.lock().connect(current_task, peer)
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        let NetlinkSocket { inner, client, message_sender: _ } = self;

        let multicast_groups = match &socket_address {
            SocketAddress::Netlink(NetlinkAddress { pid: _, groups }) => *groups,
            _ => return error!(EINVAL),
        };
        let pid = {
            let mut inner = inner.lock();
            inner.bind(current_task, socket_address)?;
            inner
                .address
                .as_ref()
                .and_then(|NetlinkAddress { pid, groups: _ }| NonZeroU32::new(*pid))
        };
        if let Some(pid) = pid {
            client.set_pid(pid);
        }
        // This "blocks" in order to synchronize with the internal
        // state of the netlink worker, but we're not blocking on
        // the completion of any i/o or any expensive computation,
        // so there's no need to support interrupts here.
        client
            .set_legacy_memberships(LegacyGroups(multicast_groups))
            .map_err(|InvalidLegacyGroupsError {}| errno!(EPERM))?
            .wait_until_complete();
        Ok(())
    }

    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        let NetlinkSocket { inner, client: _, message_sender: _ } = self;
        inner.lock().read_datagram(data, flags)
    }

    fn write(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        _dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let NetlinkSocket { inner: _, client: _, message_sender } = self;

        let bytes = data.peek_all()?;
        let bytes_len = bytes.len();

        // Parse only the netlink header to send it through security check.
        match NetlinkBuffer::new(&bytes) {
            Ok(buffer) => {
                security::check_netlink_send_access(current_task, socket, buffer.message_type())?;
            }
            Err(e) => {
                // If we can't even decode the header of the netlink message,
                // then return early here as a stronger statement that we're not
                // going to accidentally operate on it and violate the security
                // check. The netlink crate would end up dropping this with no
                // response as well.
                log_warn!(tag = NETLINK_LOG_TAG;
                    "Failed to parse netlink header {e:?}"
                );
                data.drain();
                return Ok(bytes_len);
            }
        }

        let msg = NetlinkMessageWithCreds::new(
            UnparsedNetlinkMessage::new(bytes, self.options()),
            current_task.current_creds().clone(),
        );
        message_sender.unbounded_send(msg).map_err(|e| {
            log_warn!(
                tag = NETLINK_LOG_TAG;
                "Netlink receiver unexpectedly disconnected for socket: {:?}",
                e
            );
            errno!(EPIPE)
        })?;
        data.drain();
        Ok(bytes_len)
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        let NetlinkSocket { inner, client: _, message_sender: _ } = self;
        inner.lock().wait_async(waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let NetlinkSocket { inner, client: _, message_sender: _ } = self;
        Ok(inner.lock().query_events() & FdEvents::POLLIN)
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn close(&self, _current_task: &CurrentTask, _socket: &Socket) {
        // Close the underlying channel to the Netlink worker.
        self.message_sender.close_channel();
    }

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        let NetlinkSocket { inner, client: _, message_sender: _ } = self;
        inner.lock().getsockname()
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.inner.lock().getpeername()
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        self.inner.lock().getsockopt(level, optname)
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        match (level, optname) {
            (SOL_NETLINK, NETLINK_ADD_MEMBERSHIP) => {
                let NetlinkSocket { inner: _, client, message_sender: _ } = self;
                let group: u32 = optval.read(current_task)?;
                let async_work = client
                    .add_membership(ModernGroup(group))
                    .map_err(|InvalidModernGroupError| errno!(EINVAL))?;
                // This "blocks" in order to synchronize with the internal
                // state of the rtnetlink worker, but we're not blocking on
                // the completion of any i/o or any expensive computation,
                // so there's no need to support interrupts here.
                async_work.wait_until_complete();
                Ok(())
            }
            (SOL_NETLINK, NETLINK_DROP_MEMBERSHIP) => {
                let NetlinkSocket { inner: _, client, message_sender: _ } = self;
                let group: u32 = optval.read(current_task)?;
                client
                    .del_membership(ModernGroup(group))
                    .map_err(|InvalidModernGroupError| errno!(EINVAL))?;
                Ok(())
            }
            _ => self.inner.lock().setsockopt(current_task, level, optname, optval),
        }
    }
}

/// Socket implementation for the NETLINK_GENERIC family of netlink sockets.
struct GenericNetlinkSocket {
    inner: Arc<LockDepMutex<NetlinkSocketInner, NetlinkSocketInnerLock>>,
    client: GenericNetlinkClientHandle<NetlinkToClientSender<GenericMessage>>,
    message_sender: mpsc::UnboundedSender<NetlinkMessage<GenericMessage>>,
}

impl GenericNetlinkSocket {
    pub fn new(kernel: &Kernel) -> Result<Self, Errno> {
        let inner = Arc::new(LockDepMutex::new(NetlinkSocketInner::new(NetlinkFamily::Generic)));
        let (message_sender, message_receiver) = mpsc::unbounded();
        match kernel
            .generic_netlink()
            .new_generic_client(NetlinkToClientSender::new(inner.clone()), message_receiver)
        {
            Ok(client) => Ok(Self { inner, client, message_sender }),
            Err(e) => {
                log_warn!(
                    tag = NETLINK_LOG_TAG;
                    "Failed to connect to generic netlink server. Errno: {:?}",
                    e
                );
                error!(EPIPE)
            }
        }
    }

    /// Locks and returns the inner state of the Socket.
    fn lock(&self) -> LockDepGuard<'_, NetlinkSocketInner> {
        self.inner.lock()
    }
}

impl SocketOps for GenericNetlinkSocket {
    fn connect(
        &self,
        _socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        let mut state = self.lock();
        state.connect(current_task, peer)
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        let mut state = self.lock();
        state.bind(current_task, socket_address)
    }

    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        self.lock().read_datagram(data, flags)
    }

    fn write(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        _dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let bytes = data.read_all()?;
        match NetlinkMessage::<GenericMessage>::deserialize(&bytes, EmptyDeserializeGenlOptions) {
            Err(e) => {
                log_warn!("Failed to process write; data could not be deserialized: {:?}", e);
                error!(EINVAL)
            }
            Ok(msg) => {
                security::check_netlink_send_access(current_task, socket, msg.header.message_type)?;
                match self.message_sender.unbounded_send(msg) {
                    Ok(()) => Ok(bytes.len()),
                    Err(e) => {
                        log_warn!("Netlink receiver unexpectedly disconnected for socket: {:?}", e);
                        error!(EPIPE)
                    }
                }
            }
        }
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.lock().wait_async(waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(self.lock().query_events() & FdEvents::POLLIN)
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/322875507"), "GenericNetlinkSocket::shutdown");
        Ok(())
    }

    fn close(&self, _current_task: &CurrentTask, _socket: &Socket) {}

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getsockname()
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.lock().getpeername()
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        self.lock().getsockopt(level, optname)
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        match (level, optname) {
            (SOL_NETLINK, NETLINK_ADD_MEMBERSHIP) => {
                let group_id: u32 = optval.read(current_task)?;
                self.client.add_membership(ModernGroup(group_id))
            }
            _ => self.lock().setsockopt(current_task, level, optname, optval),
        }
    }
}

/// Audit client that can be attached to the `AuditLogger`.
pub struct AuditNetlinkClient {
    /// Reference to the `AuditLogger`.
    audit_logger: Arc<AuditLogger>,
    /// The waiters queue present in `AuditNetlinkSocket`.
    waiters: WaitQueue,
    /// Optional response from the `AuditLogger`.
    audit_response:
        LockDepMutex<Option<NetlinkMessage<GenericMessage>>, AuditNetlinkClientAuditResponseLock>,
}

impl AuditNetlinkClient {
    fn new(audit_logger: Arc<AuditLogger>) -> Self {
        Self { audit_logger, waiters: Default::default(), audit_response: Default::default() }
    }

    pub fn notify(&self) {
        self.waiters.notify_fd_events(FdEvents::POLLIN);
    }

    /// Function to check the capabilities of the current task against CAP_AUDIT_*
    fn check_audit_access(
        &self,
        current_task: &CurrentTask,
        request_type: &AuditRequest,
    ) -> Result<(), Errno> {
        match request_type {
            AuditRequest::AuditGet | AuditRequest::AuditSet => {
                security::check_task_capable(current_task, CAP_AUDIT_CONTROL)
            }
            AuditRequest::AuditUser => security::check_task_capable(current_task, CAP_AUDIT_WRITE),
        }
    }

    /// Function to process request coming from userspace, it returns the response after processing
    fn process_request(
        self: &Arc<Self>,
        current_task: &CurrentTask,
        nl_message: NetlinkMessage<GenericMessage>,
    ) -> Result<NetlinkMessage<GenericMessage>, Errno> {
        let (nl_header, nl_payload) = nl_message.into_parts();
        let audit_request_type = AuditRequest::try_from(nl_header.message_type as u32)?;
        self.check_audit_access(current_task, &audit_request_type)?;

        // If there is no GenericMessage, return an ErrorMessage.
        let NetlinkPayload::InnerMessage(GenericMessage::Other { payload, .. }) = nl_payload else {
            return error!(EINVAL);
        };
        match audit_request_type {
            AuditRequest::AuditGet => self.process_get_status(nl_header.sequence_number),
            AuditRequest::AuditSet => self.process_set_status(current_task, nl_header, payload),
            AuditRequest::AuditUser => self.process_user_audit(nl_header, payload),
        }
    }

    fn get_nl_response(&self, flags: SocketMessageFlags) -> Option<Vec<u8>> {
        if flags.contains(SocketMessageFlags::PEEK) {
            if let Some(message) = self.audit_response.lock().as_ref() {
                return Some(AuditNetlinkClient::serialize_nlmsg(message.clone()));
            }
        } else if let Some(message) = self.audit_response.lock().take() {
            return Some(AuditNetlinkClient::serialize_nlmsg(message));
        }
        None
    }

    /// Function to read an audit message from `AuditLogger`.
    fn read_audit_log(self: &Arc<Self>) -> Option<Vec<u8>> {
        if let Some(AuditMessage { audit_type, message }) = self.audit_logger.read_audit_log(self) {
            return Some(AuditNetlinkClient::serialize_nlmsg(
                AuditNetlinkClient::build_audit_nlmsg(0, audit_type, message),
            ));
        }
        None
    }

    /// Function to read the optional response if present or an audit message.
    fn read_nlmsg(self: &Arc<Self>, flags: SocketMessageFlags) -> Result<Vec<u8>, Errno> {
        // First check if there is a response and send it if present.
        // Send an audit message otherwise or return EAGAIN.
        self.get_nl_response(flags).or_else(|| self.read_audit_log()).ok_or_else(|| errno!(EAGAIN))
    }

    fn process_get_status(
        &self,
        sequence_number: u32,
    ) -> Result<NetlinkMessage<GenericMessage>, Errno> {
        Ok(AuditNetlinkClient::build_audit_nlmsg(
            sequence_number,
            AUDIT_GET as u16,
            self.audit_logger.get_status().as_bytes().to_vec(),
        ))
    }

    fn process_set_status(
        self: &Arc<Self>,
        current_task: &CurrentTask,
        nl_hdr: NetlinkHeader,
        nl_payload: Vec<u8>,
    ) -> Result<NetlinkMessage<GenericMessage>, Errno> {
        let Some(status) = audit_status::read_from_bytes(nl_payload.as_bytes()).ok() else {
            return error!(EINVAL);
        };
        self.audit_logger.set_status(current_task, status, self)?;
        Ok(AuditNetlinkClient::build_audit_ack(Ok(()), nl_hdr))
    }

    fn process_user_audit(
        &self,
        nl_hdr: NetlinkHeader,
        nl_payload: Vec<u8>,
    ) -> Result<NetlinkMessage<GenericMessage>, Errno> {
        let audit_msg = String::from_utf8_lossy(nl_payload.as_bytes());
        self.audit_logger.audit_log(nl_hdr.message_type, move || audit_msg);
        Ok(AuditNetlinkClient::build_audit_ack(Ok(()), nl_hdr))
    }

    fn query_events(self: &Arc<Self>) -> FdEvents {
        if self.audit_response.lock().is_some() || self.audit_logger.get_backlog_count(self) != 0 {
            return FdEvents::POLLIN;
        }
        FdEvents::empty()
    }

    fn detach(self: &Arc<Self>) {
        self.audit_logger.detach_client(self);
    }

    fn build_audit_nlmsg(
        seq_number: u32,
        msg_type: u16,
        payload: Vec<u8>,
    ) -> NetlinkMessage<GenericMessage> {
        // The family in GenericMessage can be used for message type, not only for the Netlink Family,
        // because after finalizing the message, the message type is equal to family.
        let nl_payload =
            NetlinkPayload::InnerMessage(GenericMessage::Other { family: msg_type, payload });
        let mut nl_header = NetlinkHeader::default();
        nl_header.sequence_number = seq_number;
        let mut message = NetlinkMessage::new(nl_header, nl_payload);
        message.finalize();
        message
    }

    fn build_audit_ack(
        error: Result<(), Errno>,
        req_header: NetlinkHeader,
    ) -> NetlinkMessage<GenericMessage> {
        let error = {
            assert_eq!(req_header.buffer_len(), NETLINK_HEADER_LEN);
            let mut buffer = vec![0; NETLINK_HEADER_LEN];
            req_header.emit(&mut buffer);

            let code = match error {
                Ok(()) => None,
                Err(e) => Some(
                    // Audit netlink errors are negative.
                    NonZeroI32::new(-(e.code.error_code() as i32))
                        .expect("Errno's code must be non-zero"),
                ),
            };

            let mut error = ErrorMessage::default();
            error.code = code;
            error.header = buffer;
            error
        };

        let payload = NetlinkPayload::<GenericMessage>::Error(error);
        let mut resp_header = NetlinkHeader::default();
        resp_header.message_type = NLMSG_ERROR;
        resp_header.sequence_number = req_header.sequence_number;
        let mut message = NetlinkMessage::new(resp_header, payload);
        message.finalize();
        message
    }

    fn serialize_nlmsg(message: NetlinkMessage<GenericMessage>) -> Vec<u8> {
        let mut buf = vec![0; message.buffer_len()];
        message.serialize(&mut buf);
        buf
    }
}

/// Audit Netlink Socket structure.
pub struct AuditNetlinkSocket {
    /// Reference to the `AuditNetlinkClient` associated with self.
    audit_client: Arc<AuditNetlinkClient>,
}

impl AuditNetlinkSocket {
    pub fn new(kernel: &Kernel) -> Result<Self, Errno> {
        if kernel.audit_logger().is_disabled() {
            return error!(EPROTONOSUPPORT);
        }
        Ok(Self { audit_client: Arc::new(AuditNetlinkClient::new(kernel.audit_logger())) })
    }
}

impl SocketOps for AuditNetlinkSocket {
    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        let buf = self.audit_client.read_nlmsg(flags)?;

        let size = data.write_all(buf.as_bytes())?;
        Ok(MessageReadInfo {
            bytes_read: size,
            message_length: size,
            address: Some(SocketAddress::Netlink(NetlinkAddress::default())),
            ancillary_data: vec![],
        })
    }

    fn write(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        _dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        match NetlinkMessage::<GenericMessage>::deserialize(
            &(data.peek_all()?),
            EmptyDeserializeGenlOptions,
        ) {
            Ok(nl_message) => {
                let header = nl_message.header;
                security::check_netlink_send_access(current_task, socket, header.message_type)?;

                // Send request to the `AuditNetlinkClient`.
                let audit_ack = self
                    .audit_client
                    .process_request(current_task, nl_message)
                    .map_err(|e| AuditNetlinkClient::build_audit_ack(Err(e), header))
                    .unwrap_or_else(|nlerr| nlerr);
                *self.audit_client.audit_response.lock() = Some(audit_ack);
                data.drain();
                Ok(header.length as usize)
            }
            Err(e) => {
                log_warn!("Failed to process write; data could not be deserialized: {:?}", e);
                error!(EINVAL)
            }
        }
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.audit_client.waiters.wait_async_fd_events(waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(self.audit_client.query_events() & FdEvents::POLLIN)
    }

    fn close(&self, _current_task: &CurrentTask, _socket: &Socket) {
        // If the `AuditNetlinkClient` disconnects, detach it.
        self.audit_client.detach();
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn connect(
        &self,
        _socket: &SocketHandle,
        _current_task: &CurrentTask,
        _peer: SocketPeer,
    ) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(EOPNOTSUPP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        error!(EOPNOTSUPP)
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        error!(EOPNOTSUPP)
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _level: u32,
        _optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        error!(EOPNOTSUPP)
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        _level: u32,
        _optname: u32,
        _optval: SockOptValue,
    ) -> Result<(), Errno> {
        error!(EOPNOTSUPP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use netlink_packet_route::route::RouteMessage;
    use netlink_packet_route::{RouteNetlinkMessage, RouteNetlinkMessageParseMode};
    use test_case::test_case;

    // Successfully send the message and observe it's stored in the queue.
    #[test_case(true; "sufficient_capacity")]
    // Attempting to send when the queue is full should succeed by increasing
    // the size of the queue.
    #[test_case(false; "insufficient_capacity")]
    fn test_netlink_to_client_sender(sufficient_capacity: bool) {
        const MODERN_GROUP: u32 = 5;

        let mut message: NetlinkMessage<RouteNetlinkMessage> =
            RouteNetlinkMessage::NewRoute(RouteMessage::default()).into();
        message.finalize();

        let (initial_queue_size, final_queue_size) = if sufficient_capacity {
            (SOCKET_DEFAULT_SIZE, SOCKET_DEFAULT_SIZE)
        } else {
            (0, message.buffer_len())
        };

        let socket_inner = Arc::new(LockDepMutex::new(NetlinkSocketInner {
            receive_buffer: MessageQueue::new(initial_queue_size),
            ..NetlinkSocketInner::new(NetlinkFamily::Route)
        }));

        let mut sender = NetlinkToClientSender::<RouteNetlinkMessage>::new(socket_inner.clone());
        sender.send(message.clone(), Some(ModernGroup(MODERN_GROUP)));
        let Message { data, address, ancillary_data: _ } =
            socket_inner.lock().read_message().expect("should read message");

        assert_eq!(
            address,
            Some(SocketAddress::Netlink(NetlinkAddress { pid: 0, groups: 1 << MODERN_GROUP }))
        );
        let actual_message = NetlinkMessage::<RouteNetlinkMessage>::deserialize(
            &data,
            RouteNetlinkMessageParseMode::Strict,
        )
        .expect("message should deserialize into RtnlMessage");
        assert_eq!(actual_message, message);
        assert_eq!(socket_inner.lock().receive_buffer.capacity(), final_queue_size);
    }

    fn getsockopt_u32(socket: &NetlinkSocketInner, level: u32, optname: u32) -> u32 {
        let byte_vec = socket.getsockopt(level, optname).expect("getsockopt should succeed");
        let bytes: [u8; 4] = byte_vec.as_slice().try_into().expect("expected 4 bytes");
        u32::from_ne_bytes(bytes)
    }

    fn sock_opt_value(val: u32) -> SockOptValue {
        SockOptValue::Value(val.to_ne_bytes().to_vec())
    }

    #[::fuchsia::test]
    async fn test_set_get_snd_rcv_buf() {
        crate::testing::spawn_kernel_and_run_sync(|current_task| {
            let mut socket = NetlinkSocketInner::new(NetlinkFamily::Route);

            // Verify initialization uses the default value.
            let expected_default = u32::try_from(SOCKET_DEFAULT_SIZE).unwrap();
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_SNDBUF), expected_default);
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_RCVBUF), expected_default);

            // Set new values and observe that they were applied.
            // Note that applied value is 2 times the requested value.
            const SNDBUF_SIZE: u32 = 12345;
            const RCVBUF_SIZE: u32 = 54321;
            socket
                .setsockopt(current_task, SOL_SOCKET, SO_SNDBUF, sock_opt_value(SNDBUF_SIZE))
                .expect("setsockopt should succeed");
            socket
                .setsockopt(current_task, SOL_SOCKET, SO_RCVBUF, sock_opt_value(RCVBUF_SIZE))
                .expect("setsockopt should succeed");
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_SNDBUF), SNDBUF_SIZE * 2);
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_RCVBUF), RCVBUF_SIZE * 2);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_snd_rcv_buf_limits() {
        crate::testing::spawn_kernel_and_run_sync(|current_task| {
            let mut socket = NetlinkSocketInner::new(NetlinkFamily::Route);
            let too_big = u32::try_from(SOCKET_MAX_SIZE).unwrap() + 1;

            // SO_SNDBUF and SO_RCVBUF clamp the size to the limit.
            socket
                .setsockopt(current_task, SOL_SOCKET, SO_SNDBUF, sock_opt_value(too_big))
                .expect("setsockopt should succeed");
            socket
                .setsockopt(current_task, SOL_SOCKET, SO_RCVBUF, sock_opt_value(too_big))
                .expect("setsockopt should succeed");
            let expected_max = u32::try_from(SOCKET_MAX_SIZE).unwrap();
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_SNDBUF), expected_max);
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_RCVBUF), expected_max);

            // SO_SNDBUFFORCE and SO_RCVBUFFORCE do not.
            // Note that the applied value is two times the requested value.
            socket
                .setsockopt(current_task, SOL_SOCKET, SO_SNDBUFFORCE, sock_opt_value(too_big))
                .expect("setsockopt should succeed");
            socket
                .setsockopt(current_task, SOL_SOCKET, SO_RCVBUFFORCE, sock_opt_value(too_big))
                .expect("setsockopt should succeed");
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_SNDBUF), too_big * 2);
            assert_eq!(getsockopt_u32(&socket, SOL_SOCKET, SO_RCVBUF), too_big * 2);
        })
        .await;
    }
}
