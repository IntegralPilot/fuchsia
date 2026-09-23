// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bpf::attachments::{SockAddrOp, SockAddrProgramResult, SockOp, SockProgramResult};
use crate::fs::fuchsia::zxio::{zxio_query_events, zxio_wait_async};
use crate::mm::MemoryAccessorExt;
use crate::security;
use crate::task::syscalls::SockFProgPtr;
use crate::task::{CurrentTask, EventHandler, Kernel, Task, WaitCanceler, Waiter};
use crate::vfs::socket::socket::ReadFromSockOptValue as _;
use crate::vfs::socket::{
    SockOptValue, Socket, SocketAddress, SocketDomain, SocketHandle, SocketMessageFlags, SocketOps,
    SocketPeer, SocketProtocol, SocketShutdownFlags, SocketType,
};
use crate::vfs::{AncillaryData, FileObject, InputBuffer, MessageReadInfo, OutputBuffer};
use byteorder::ByteOrder;
use ebpf::convert_and_verify_cbpf;
use ebpf_api::SOCKET_FILTER_CBPF_CONFIG;
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use linux_uapi::{IP_MULTICAST_ALL, IP_PASSSEC};
use starnix_logging::{log_error, log_warn, track_stub};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::auth::{CAP_NET_ADMIN, CAP_NET_RAW};
use starnix_uapi::errors::{ENOTSUP, Errno, ErrnoCode};
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    AF_PACKET, BPF_MAXINSNS, FIONREAD, MSG_DONTWAIT, MSG_WAITALL, SO_ATTACH_FILTER,
    SO_BINDTODEVICE, SO_BINDTOIFINDEX, SO_COOKIE, c_int, errno, errno_from_zxio_code, error,
    from_status_like_fdio, sock_filter, uapi, ucred, uid_t,
};
use static_assertions::const_assert_eq;
use std::mem::size_of;
use std::sync::{Arc, OnceLock};
use syncio::zxio::{
    IP_RECVERR, IP_TRANSPARENT, SO_DOMAIN, SO_FUCHSIA_MARK, SO_MARK, SO_PROTOCOL, SO_REUSEPORT,
    SO_TYPE, SOL_IP, SOL_SOCKET, ZXIO_SOCKET_MARK_DOMAIN_1, ZXIO_SOCKET_MARK_DOMAIN_2,
    zxio_socket_mark,
};
use syncio::{
    ControlMessage, RecvMessageInfo, ServiceConnector, Zxio, ZxioErrorCode,
    ZxioSocketCreationOptions, ZxioSocketMark, ZxioWakeGroupToken,
};
use zerocopy::IntoBytes;

/// Linux marks aren't compatible with Fuchsia marks, we store the `SO_MARK`
/// value in the fuchsia `ZXIO_SOCKET_MARK_DOMAIN_1`. If a mark in this domain
/// is absent, it will be reported to starnix applications as a `0` since that
/// is the default mark value on Linux.
pub const ZXIO_SOCKET_MARK_SO_MARK: u8 = ZXIO_SOCKET_MARK_DOMAIN_1;
/// Fuchsia does not have uids, we use the `ZXIO_SOCKET_MARK_DOMAIN_2` on the
/// socket to store the UID for the sockets created by starnix.
pub const ZXIO_SOCKET_MARK_UID: u8 = ZXIO_SOCKET_MARK_DOMAIN_2;

/// Connects to the appropriate `fuchsia_posix_socket_*::Provider` protocol.
struct SocketProviderServiceConnector;

impl ServiceConnector for SocketProviderServiceConnector {
    fn connect(service_name: &str) -> Result<&'static zx::Channel, zx::Status> {
        match service_name {
            fposix_socket::ProviderMarker::PROTOCOL_NAME => {
                static CHANNEL: OnceLock<Result<zx::Channel, zx::Status>> = OnceLock::new();
                &CHANNEL
            }
            fposix_socket_packet::ProviderMarker::PROTOCOL_NAME => {
                static CHANNEL: OnceLock<Result<zx::Channel, zx::Status>> = OnceLock::new();
                &CHANNEL
            }
            fposix_socket_raw::ProviderMarker::PROTOCOL_NAME => {
                static CHANNEL: OnceLock<Result<zx::Channel, zx::Status>> = OnceLock::new();
                &CHANNEL
            }
            _ => return Err(zx::Status::INTERNAL),
        }
        .get_or_init(|| {
            let (client, server) = zx::Channel::create();
            let protocol_path = format!("/svc/{service_name}");
            fdio::service_connect(&protocol_path, server)?;
            Ok(client)
        })
        .as_ref()
        .map_err(|status| *status)
    }
}

// Trait for types that can be converted to a byte vector that contains a
// `sockaddr` value.
trait AsSockAddrBytes {
    fn as_sockaddr_bytes(&self) -> Result<&[u8], Errno>;
}

impl AsSockAddrBytes for &SocketAddress {
    fn as_sockaddr_bytes(&self) -> Result<&[u8], Errno> {
        match self {
            SocketAddress::Inet(addr) => Ok(&addr[..]),
            SocketAddress::Inet6(addr) => Ok(&addr[..]),
            _ => error!(EAFNOSUPPORT),
        }
    }
}

impl AsSockAddrBytes for &Vec<u8> {
    fn as_sockaddr_bytes(&self) -> Result<&[u8], Errno> {
        Ok(self.as_slice())
    }
}

/// A socket backed by an underlying Zircon I/O object.
pub struct ZxioBackedSocket {
    /// The underlying Zircon I/O object.
    zxio: syncio::Zxio,

    // SO_COOKIE cache.
    cookie: OnceLock<u64>,

    // Token resolver for this socket.
    token_resolver: Arc<SocketTokenResolver>,

    // UID of the process that created socket.
    uid: uid_t,
}

impl ZxioBackedSocket {
    pub fn new(
        current_task: &CurrentTask,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
    ) -> Result<ZxioBackedSocket, Errno> {
        let marks = &mut [
            ZxioSocketMark::so_mark(0),
            ZxioSocketMark::uid(current_task.current_creds().uid),
        ];

        match (domain, socket_type, protocol) {
            (SocketDomain::Inet, SocketType::Datagram, SocketProtocol::ICMP)
            | (SocketDomain::Inet6, SocketType::Datagram, SocketProtocol::ICMPV6) => {
                let gid_range =
                    current_task.kernel().system_limits.socket.icmp_ping_gids.lock().clone();
                if !gid_range.contains(&current_task.current_creds().egid) {
                    return error!(EACCES);
                }
            }
            _ => (),
        };

        let wake_group = match current_task.kernel().netstack_wake_group() {
            Some(fnet_resources::WakeGroupToken { token }) => token
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("duplicate handle to wake group"),
            None => {
                log_warn!("Netstack wake group not available. Failing socket creation");
                // Make applications fail loudly because this shouldn't happen
                // unless something is misconfigured or the netstack has died.
                return error!(ENOTRECOVERABLE);
            }
        };

        let zxio = Zxio::new_socket::<SocketProviderServiceConnector>(
            domain.as_raw() as c_int,
            socket_type.as_raw() as c_int,
            protocol.as_raw() as c_int,
            ZxioSocketCreationOptions {
                marks,
                wake_group: ZxioWakeGroupToken::new(Some(wake_group)),
            },
        )
        .map_err(|status| from_status_like_fdio!(status))?
        .map_err(|out_code| errno_from_zxio_code!(out_code))?;

        let socket = Self::new_with_zxio(current_task, zxio);

        if matches!(domain, SocketDomain::Inet | SocketDomain::Inet6) {
            match current_task.kernel().ebpf_state.attachments.root_cgroup().run_sock_prog(
                current_task,
                SockOp::Create,
                domain,
                socket_type,
                protocol,
                &socket,
            ) {
                SockProgramResult::Allow => (),
                SockProgramResult::Block => return error!(EPERM),
            }
        }

        Ok(socket)
    }

    pub fn new_with_zxio(current_task: &CurrentTask, zxio: syncio::Zxio) -> ZxioBackedSocket {
        let uid = current_task.current_creds().euid;
        let token_resolver = current_task
            .kernel()
            .socket_tokens_store
            .get_token_resolver(current_task.kernel(), uid);
        ZxioBackedSocket { zxio, cookie: Default::default(), token_resolver, uid }
    }

    fn sendmsg(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        addr: &Option<SocketAddress>,
        data: &mut dyn InputBuffer,
        cmsgs: Vec<ControlMessage>,
        flags: SocketMessageFlags,
    ) -> Result<usize, Errno> {
        let mut addr = match addr {
            Some(
                SocketAddress::Inet(sockaddr)
                | SocketAddress::Inet6(sockaddr)
                | SocketAddress::Packet(sockaddr),
            ) => sockaddr.clone(),
            Some(_) => return error!(EINVAL),
            None => vec![],
        };

        // Run `CGROUP_UDP[46]_SENDMSG` eBPF programs for `sendto()` and
        // `sendmsg()` on UDP sockets. Not necessary for `send()` (i.e. when
        // `addr` is empty).
        if matches!(
            (socket.domain, socket.socket_type),
            (SocketDomain::Inet | SocketDomain::Inet6, SocketType::Datagram)
        ) && addr.len() > 0
        {
            self.run_sockaddr_ebpf(socket, current_task, SockAddrOp::UdpSendMsg, &addr)?;
        }

        let map_errors = |res: Result<Result<usize, ZxioErrorCode>, zx::Status>| {
            res.map_err(|status| match status {
                zx::Status::OUT_OF_RANGE => errno!(EMSGSIZE),
                other => from_status_like_fdio!(other),
            })?
            .map_err(|out_code| errno_from_zxio_code!(out_code))
        };

        let flags = flags.bits() & !MSG_DONTWAIT;
        let sent_bytes = match data.peek_all_segments_as_iovecs() {
            Ok(mut iovecs) => {
                // Note: We have to prefault here because this is a C FFI call and we cannot
                // catch faults directly like we do for Starnix-internal usercopies.
                // In the future, we could look into implementing reactive faulting in
                // `zxio_maybe_faultable_copy_impl` to match the behavior of internal
                // usercopies.
                let ranges =
                    iovecs.as_ref().iter().filter(|iovec| iovec.iov_len > 0).map(|iovec| {
                        (UserAddress::from_ptr(iovec.iov_base as usize), Some(iovec.iov_len))
                    });
                if let Ok(mm) = current_task.mm() {
                    mm.ensure_ranges_mapped_in_user_vmar(ranges)?;
                }

                Some(map_errors(self.zxio.sendmsg(&mut addr, &mut iovecs, &cmsgs, flags))?)
            }
            Err(e) if e.code == ENOTSUP => None,
            Err(e) => return Err(e),
        };

        // If we can't pass the iovecs directly so fallback to reading
        // all the bytes from the input buffer first.
        let sent_bytes = match sent_bytes {
            Some(sent_bytes) => sent_bytes,
            None => {
                let mut bytes = data.peek_all()?;
                map_errors(self.zxio.sendmsg(
                    &mut addr,
                    &mut [syncio::zxio::iovec {
                        iov_base: bytes.as_mut_ptr() as *mut starnix_uapi::c_void,
                        iov_len: bytes.len(),
                    }],
                    &cmsgs,
                    flags,
                ))?
            }
        };
        data.advance(sent_bytes)?;
        Ok(sent_bytes)
    }

    fn recvmsg(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<RecvMessageInfo, Errno> {
        let flags = flags.bits() & !MSG_DONTWAIT & !MSG_WAITALL;

        let map_errors = |res: Result<Result<RecvMessageInfo, ZxioErrorCode>, zx::Status>| {
            res.map_err(|status| from_status_like_fdio!(status))?
                .map_err(|out_code| errno_from_zxio_code!(out_code))
        };

        let info = match data.peek_all_segments_as_iovecs() {
            Ok(mut iovecs) => {
                // Note: We have to prefault here because this is a C FFI call and we cannot
                // catch faults directly like we do for Starnix-internal usercopies.
                // In the future, we could look into implementing reactive faulting in
                // `zxio_maybe_faultable_copy_impl` to match the behavior of internal
                // usercopies.
                let ranges =
                    iovecs.as_ref().iter().filter(|iovec| iovec.iov_len > 0).map(|iovec| {
                        (UserAddress::from_ptr(iovec.iov_base as usize), Some(iovec.iov_len))
                    });
                if let Ok(mm) = current_task.mm() {
                    mm.ensure_ranges_mapped_in_user_vmar(ranges)?;
                }

                let info = map_errors(self.zxio.recvmsg(&mut iovecs, flags))?;
                // SAFETY: we successfully read `info.bytes_read` bytes
                // directly to the user's buffer segments.
                (unsafe { data.advance(info.bytes_read) })?;
                Some(info)
            }
            Err(e) if e.code == ENOTSUP => None,
            Err(e) => return Err(e),
        };

        // If we can't pass the segments directly, fallback to receiving
        // all the bytes in an intermediate buffer and writing that
        // to our output buffer.
        let info = match info {
            Some(info) => info,
            None => {
                // TODO: use MaybeUninit
                let mut buf = vec![0; data.available()];
                let iovec = &mut [syncio::zxio::iovec {
                    iov_base: buf.as_mut_ptr() as *mut starnix_uapi::c_void,
                    iov_len: buf.len(),
                }];
                let info = map_errors(self.zxio.recvmsg(iovec, flags))?;
                let written = data.write_all(&buf[..info.bytes_read])?;
                debug_assert_eq!(written, info.bytes_read);
                info
            }
        };

        // Run eBPF programs for UDP sockets.
        if matches!(
            (socket.domain, socket.socket_type),
            (SocketDomain::Inet | SocketDomain::Inet6, SocketType::Datagram)
        ) {
            self.run_sockaddr_ebpf(socket, current_task, SockAddrOp::UdpRecvMsg, &info.address)?;
        }

        Ok(info)
    }

    fn attach_cbpf_filter(&self, _task: &Task, code: Vec<sock_filter>) -> Result<(), Errno> {
        // SO_ATTACH_FILTER is supported only for packet sockets.
        let domain = self
            .zxio
            .getsockopt(SOL_SOCKET, SO_DOMAIN, size_of::<u32>() as u32)
            .map_err(|status| from_status_like_fdio!(status))?
            .map_err(|out_code| errno_from_zxio_code!(out_code))?;
        let domain = u32::from_ne_bytes(domain.try_into().unwrap());
        if domain != u32::from(AF_PACKET) {
            return error!(ENOTSUP);
        }

        let program = convert_and_verify_cbpf(
            &code,
            ebpf_api::SOCKET_FILTER_SK_BUF_TYPE.clone(),
            &SOCKET_FILTER_CBPF_CONFIG,
        )
        .map_err(|_| errno!(EINVAL))?;

        // TODO(https://fxbug.dev/377332291) Use `zxio_borrow()` to avoid cloning the handle.
        let packet_socket = fidl::endpoints::ClientEnd::<fposix_socket_packet::SocketMarker>::new(
            self.zxio.clone_handle().map_err(|_| errno!(EIO))?.into(),
        )
        .into_sync_proxy();
        let code = program.to_code();
        let code: &[u64] = zerocopy::transmute_ref!(code.as_slice());
        let result = packet_socket.attach_bpf_filter_unsafe(code, zx::MonotonicInstant::INFINITE);
        result.map_err(|_: fidl::Error| errno!(EIO))?.map_err(|e| {
            Errno::with_context(
                ErrnoCode::from_error_code(e.into_primitive() as i16),
                "AttachBfpFilterUnsafe",
            )
        })
    }

    fn run_sockaddr_ebpf(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        op: SockAddrOp,
        socket_address: impl AsSockAddrBytes,
    ) -> Result<(), Errno> {
        // BPF_PROG_TYPE_CGROUP_SOCK_ADDR programs are executed only for IPv4 and IPv6 sockets.
        if !matches!(socket.domain, SocketDomain::Inet | SocketDomain::Inet6) {
            return Ok(());
        }

        let ebpf_result =
            current_task.kernel().ebpf_state.attachments.root_cgroup().run_sock_addr_prog(
                current_task,
                op,
                socket.domain,
                socket.socket_type,
                socket.protocol,
                socket_address.as_sockaddr_bytes()?,
                socket,
            )?;
        match ebpf_result {
            SockAddrProgramResult::Allow => Ok(()),
            SockAddrProgramResult::Block => error!(EPERM),
        }
    }

    pub fn get_socket_cookie(&self) -> Result<u64, Errno> {
        if let Some(cookie) = self.cookie.get() {
            return Ok(*cookie);
        }

        let cookie = u64::from_ne_bytes(
            self.zxio
                .getsockopt(SOL_SOCKET, SO_COOKIE, size_of::<u64>() as u32)
                .map_err(|status| from_status_like_fdio!(status))?
                .map_err(|out_code| errno_from_zxio_code!(out_code))?
                .try_into()
                .unwrap(),
        );
        let _: Result<(), u64> = self.cookie.set(cookie);

        return Ok(cookie);
    }

    pub fn uid(&self) -> uid_t {
        self.uid
    }
}

impl SocketOps for ZxioBackedSocket {
    fn get_socket_info(&self) -> Result<(SocketDomain, SocketType, SocketProtocol), Errno> {
        let getsockopt = |optname: u32| -> Result<u32, Errno> {
            Ok(u32::from_ne_bytes(
                self.zxio
                    .getsockopt(SOL_SOCKET, optname, size_of::<u32>() as u32)
                    .map_err(|status| from_status_like_fdio!(status))?
                    .map_err(|out_code| errno_from_zxio_code!(out_code))?
                    .try_into()
                    .unwrap(),
            ))
        };

        let domain_raw = getsockopt(SO_DOMAIN)?;
        let domain = SocketDomain::from_raw(domain_raw.try_into().map_err(|_| errno!(EINVAL))?)
            .ok_or_else(|| errno!(EINVAL))?;

        let type_raw = getsockopt(SO_TYPE)?;
        let socket_type = SocketType::from_raw(type_raw).ok_or_else(|| errno!(EINVAL))?;

        let protocol_raw = getsockopt(SO_PROTOCOL)?;
        let protocol = SocketProtocol::from_raw(protocol_raw);

        Ok((domain, socket_type, protocol))
    }

    fn connect(
        &self,
        socket: &SocketHandle,
        current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        match peer {
            SocketPeer::Address(
                ref address @ (SocketAddress::Inet(_) | SocketAddress::Inet6(_)),
            ) => self.run_sockaddr_ebpf(socket, current_task, SockAddrOp::Connect, address)?,
            _ => (),
        };

        let connect_result = match &peer {
            SocketPeer::Address(
                SocketAddress::Inet(addr)
                | SocketAddress::Inet6(addr)
                | SocketAddress::Packet(addr),
            ) => self
                .zxio
                .connect(addr)
                .map_err(|status| from_status_like_fdio!(status))?
                .map_err(|out_code| errno_from_zxio_code!(out_code)),
            SocketPeer::Address(SocketAddress::Unspecified) => {
                if socket.socket_type == SocketType::Datagram {
                    let unspec = uapi::sockaddr::default().as_bytes().to_vec();
                    let res = self
                        .zxio
                        .connect(&unspec)
                        .map_err(|status| from_status_like_fdio!(status))?
                        .map_err(|out_code| errno_from_zxio_code!(out_code));
                    match res {
                        Ok(()) => Ok(()),
                        Err(err)
                            if err == errno!(EINVAL)
                                || err == errno!(ENOTCONN)
                                || err == errno!(EAFNOSUPPORT) =>
                        {
                            Ok(())
                        }
                        Err(err) => Err(err),
                    }
                } else {
                    error!(EAFNOSUPPORT)
                }
            }
            _ => error!(EINVAL),
        };

        if connect_result.is_ok() {
            if let SocketPeer::Address(ref address) = peer {
                if let Some(port) = address.maybe_inet_port() {
                    if port == 80 {
                        track_stub!(
                            TODO("https://fxbug.dev/540841946"),
                            "fake NFLOG message on port 80 connect"
                        );
                        let uid = current_task.current_creds().uid;
                        crate::vfs::socket::send_fake_nflog_message(uid);
                    }
                }
            }
        }

        connect_result
    }

    fn listen(&self, _socket: &Socket, backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        self.zxio
            .listen(backlog)
            .map_err(|status| from_status_like_fdio!(status))?
            .map_err(|out_code| errno_from_zxio_code!(out_code))
    }

    fn accept(&self, socket: &Socket, current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        let zxio = self
            .zxio
            .accept()
            .map_err(|status| from_status_like_fdio!(status))?
            .map_err(|out_code| errno_from_zxio_code!(out_code))?;

        let accepted = Self::new_with_zxio(current_task, zxio);

        // Clone sk_storage entries marked with BPF_F_CLONE.
        fn log_cookie_error(err: &Errno) {
            log_error!("Failed to get socket cookie: {err:?}");
        }
        let parent_cookie = self.get_socket_cookie().inspect_err(log_cookie_error);
        let child_cookie = accepted.get_socket_cookie().inspect_err(log_cookie_error);
        if let (Ok(parent_cookie), Ok(child_cookie)) = (parent_cookie, child_cookie) {
            current_task.kernel().ebpf_state.clone_sk_storage_entries(parent_cookie, child_cookie);
        }

        Ok(Socket::new_with_ops_and_info(
            Box::new(accepted),
            socket.domain,
            socket.socket_type,
            socket.protocol,
        ))
    }

    fn bind(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        self.run_sockaddr_ebpf(socket, current_task, SockAddrOp::Bind, &socket_address)?;

        match socket_address {
            SocketAddress::Inet(addr)
            | SocketAddress::Inet6(addr)
            | SocketAddress::Packet(addr) => self
                .zxio
                .bind(&addr)
                .map_err(|status| from_status_like_fdio!(status))?
                .map_err(|out_code| errno_from_zxio_code!(out_code)),
            _ => error!(EINVAL),
        }
    }

    fn read(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        // MSG_ERRQUEUE is not supported for TCP sockets, but it's expected to fail with EAGAIN.
        if socket.socket_type == SocketType::Stream && flags.contains(SocketMessageFlags::ERRQUEUE)
        {
            return error!(EAGAIN);
        }

        let mut info = self.recvmsg(socket, current_task, data, flags)?;

        let bytes_read = info.bytes_read;

        let address = if !info.address.is_empty() {
            Some(SocketAddress::from_bytes(info.address)?)
        } else {
            None
        };

        Ok(MessageReadInfo {
            bytes_read,
            message_length: info.message_length,
            address,
            ancillary_data: info.control_messages.drain(..).map(AncillaryData::Ip).collect(),
        })
    }

    fn write(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        dest_address: &mut Option<SocketAddress>,
        ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let mut cmsgs = vec![];
        for d in ancillary_data.drain(..) {
            match d {
                AncillaryData::Ip(msg) => cmsgs.push(msg),
                _ => return error!(EINVAL),
            }
        }

        // Ignore destination address if this is a stream socket.
        let dest_address =
            if socket.socket_type == SocketType::Stream { &None } else { dest_address };
        self.sendmsg(socket, current_task, dest_address, data, cmsgs, SocketMessageFlags::empty())
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        zxio_wait_async(&self.zxio, waiter, events, handler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        zxio_query_events(&self.zxio)
    }

    fn shutdown(&self, _socket: &Socket, how: SocketShutdownFlags) -> Result<(), Errno> {
        self.zxio
            .shutdown(how)
            .map_err(|status| from_status_like_fdio!(status))?
            .map_err(|out_code| errno_from_zxio_code!(out_code))
    }

    fn close(&self, current_task: &CurrentTask, socket: &Socket) {
        if matches!(socket.domain, SocketDomain::Inet | SocketDomain::Inet6) {
            // Invoke eBPF release program (if any). Result is ignored since we cannot block
            // socket release.
            let _: SockProgramResult =
                current_task.kernel().ebpf_state.attachments.root_cgroup().run_sock_prog(
                    current_task,
                    SockOp::Release,
                    socket.domain,
                    socket.socket_type,
                    socket.protocol,
                    self,
                );
        }

        let cookie = self.get_socket_cookie();

        let _ = self.zxio.close();

        // TODO(https://fxbug.dev/496639039): Move sk_storage cleanup to Netstack.
        if let Ok(cookie) = cookie {
            current_task.kernel().ebpf_state.remove_sk_storage_entries(cookie);
        }
    }

    fn getsockname(&self, socket: &Socket) -> Result<SocketAddress, Errno> {
        match self.zxio.getsockname() {
            Err(_) | Ok(Err(_)) => Ok(SocketAddress::default_for_domain(socket.domain)),
            Ok(Ok(addr)) => SocketAddress::from_bytes(addr),
        }
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        self.zxio
            .getpeername()
            .map_err(|status| from_status_like_fdio!(status))?
            .map_err(|out_code| errno_from_zxio_code!(out_code))
            .and_then(SocketAddress::from_bytes)
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
            (SOL_SOCKET, SO_ATTACH_FILTER) => {
                let fprog = SockFProgPtr::read_from_sockopt_value(current_task, &optval)?;
                if fprog.len > BPF_MAXINSNS || fprog.len == 0 {
                    return error!(EINVAL);
                }
                let code: Vec<sock_filter> = current_task
                    .read_multi_arch_objects_to_vec(fprog.filter, fprog.len as usize)?;
                return self.attach_cbpf_filter(current_task, code);
            }
            (SOL_IP, IP_RECVERR) => {
                track_stub!(TODO("https://fxbug.dev/333060595"), "SOL_IP.IP_RECVERR");
                return Ok(());
            }
            (SOL_IP, IP_MULTICAST_ALL) => {
                track_stub!(TODO("https://fxbug.dev/404596095"), "SOL_IP.IP_MULTICAST_ALL");
                return Ok(());
            }
            (SOL_IP, IP_PASSSEC) if current_task.kernel().features.selinux_test_suite => {
                track_stub!(TODO("https://fxbug.dev/398663317"), "SOL_IP.IP_PASSSEC");
                return Ok(());
            }
            (SOL_SOCKET, SO_MARK) => {
                // Either `CAP_NET_RAW` or `CAP_NET_ADMIN` is required to set
                // `SO_MARK`. If `CAP_NET_RAW` is not present, we then check
                // `CAP_NET_ADMIN` using `check_task_capable`, which will
                // generate an audit record if the capability is missing.
                if !security::is_task_capable_noaudit(current_task, CAP_NET_RAW) {
                    security::check_task_capable(current_task, CAP_NET_ADMIN)?;
                }

                let mark: u32 = optval.read(current_task)?;
                let socket_mark = ZxioSocketMark::so_mark(mark);
                let optval: &[u8; size_of::<zxio_socket_mark>()] =
                    zerocopy::transmute_ref!(&socket_mark);
                return self
                    .zxio
                    .setsockopt(SOL_SOCKET as i32, SO_FUCHSIA_MARK as i32, optval, None)
                    .map_err(|status| from_status_like_fdio!(status))?
                    .map_err(|out_code| errno_from_zxio_code!(out_code));
            }
            (SOL_SOCKET, SO_BINDTODEVICE | SO_BINDTOIFINDEX) => {
                // Require `CAP_NET_RAW` to bind the socket to a device. This
                // is consistent with Linux prior to 5.7. Starting from 5.7
                // Linux allows requires this capability only if the socket
                // is already bound to a device.
                security::check_task_capable(current_task, CAP_NET_RAW).inspect_err(|_| {
                    log_warn!(
                        "setsockopt(SO_BINDTODEVICE) is called by a \
                         process without CAP_NET_RAW: {:?}",
                        current_task.thread_group(),
                    )
                })?;
            }
            (SOL_IP, IP_TRANSPARENT) => {
                // Either `CAP_NET_RAW` or `CAP_NET_ADMIN` is required to set
                // `IP_TRANSPARENT`. If `CAP_NET_RAW` is not present, we then
                // check `CAP_NET_ADMIN` using `check_task_capable`, which will
                // generate an audit record if the capability is missing.
                if !security::is_task_capable_noaudit(current_task, CAP_NET_RAW) {
                    security::check_task_capable(current_task, CAP_NET_ADMIN)?;
                }
            }
            _ => {}
        }

        let optval = optval.to_vec(current_task)?;

        let access_token = match (level, optname) {
            (SOL_SOCKET, SO_REUSEPORT) => Some(self.token_resolver.get_sharing_domain_token()),
            _ => None,
        };

        self.zxio
            .setsockopt(level as i32, optname as i32, &optval, access_token)
            .map_err(|status| from_status_like_fdio!(status))?
            .map_err(|out_code| errno_from_zxio_code!(out_code))
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        match (level, optname) {
            // SO_MARK is specialized because linux socket marks are not compatible
            // with fuchsia socket marks. We need to get the socket mark from the
            // `ZXIO_SOCKET_MARK_SO_MARK` domain.
            (SOL_SOCKET, SO_MARK) => {
                let mut optval: [u8; size_of::<zxio_socket_mark>()] =
                    zerocopy::try_transmute!(zxio_socket_mark {
                        is_present: false,
                        domain: fidl_fuchsia_net::MARK_DOMAIN_SO_MARK as u8,
                        value: 0,
                        ..Default::default()
                    })
                    .expect("invalid bit pattern");
                // Retrieves the `zxio_socket_mark` from the domain.
                let optlen = self
                    .zxio
                    .getsockopt_slice(level, SO_FUCHSIA_MARK, &mut optval)
                    .map_err(|status| from_status_like_fdio!(status))?
                    .map_err(|out_code| errno_from_zxio_code!(out_code))?;
                if optlen as usize != size_of::<zxio_socket_mark>() {
                    return error!(EINVAL);
                }
                let socket_mark: zxio_socket_mark =
                    zerocopy::try_transmute!(optval).map_err(|_validity_err| errno!(EINVAL))?;
                // Translate to a linux mark, the default value is 0.
                let mark = if socket_mark.is_present { socket_mark.value } else { 0 };
                let mut result = vec![0; 4];
                byteorder::NativeEndian::write_u32(&mut result, mark);
                Ok(result)
            }
            (SOL_SOCKET, SO_COOKIE) => {
                self.get_socket_cookie().map(|cookie| cookie.as_bytes().to_owned())
            }
            (SOL_IP, IP_MULTICAST_ALL) => {
                track_stub!(TODO("https://fxbug.dev/404596095"), "SOL_IP.IP_MULTICAST_ALL");
                let mut result = vec![0; 4];
                byteorder::NativeEndian::write_u32(&mut result, 0);
                Ok(result)
            }
            _ => self
                .zxio
                .getsockopt(level, optname, optlen)
                .map_err(|status| from_status_like_fdio!(status))?
                .map_err(|out_code| errno_from_zxio_code!(out_code)),
        }
    }

    fn to_handle(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        self.zxio
            .deep_clone()
            .and_then(Zxio::release)
            .map(Some)
            .map_err(|status| from_status_like_fdio!(status))
    }

    fn ioctl(
        &self,
        socket: &Socket,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let user_addr = UserAddress::from(arg);
        match request {
            FIONREAD if socket.socket_type == SocketType::Stream => {
                let available = self
                    .zxio
                    .get_read_buffer_available()
                    .map_err(|status| from_status_like_fdio!(status))?;
                let available: i32 = available.try_into().map_err(|_| errno!(EINVAL))?;
                current_task.write_object(UserRef::<i32>::new(user_addr), &available)?;
                Ok(SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}

pub use tokens_store::SocketTokensStore;
type SocketTokenResolver = tokens_store::SocketTokenResolver<Kernel>;

mod tokens_store {
    use crate::task::Kernel;
    use derivative::Derivative;
    use fuchsia_async as fasync;
    use fuchsia_rcu::RcuDroppable;
    use starnix_rcu::RcuHashMap;
    use starnix_rcu::rcu_hash_map::Entry;
    use starnix_uapi::uid_t;
    use std::sync::{Arc, OnceLock, Weak};

    /// Trait for the `Kernel` functionality used in `SocketTokensStore`. Mocked
    /// in tests.
    pub trait SocketTokenStoreHost: Sized + Sync + Send + 'static {
        fn get_socket_tokens_store(&self) -> &SocketTokensStore<Self>;
        fn spawn_future(&self, future: impl AsyncFnOnce() -> () + Send + 'static);
    }

    impl SocketTokenStoreHost for Kernel {
        fn get_socket_tokens_store(&self) -> &SocketTokensStore<Self> {
            &self.socket_tokens_store
        }
        fn spawn_future(&self, future: impl AsyncFnOnce() -> () + Send + 'static) {
            self.kthreads.spawn_future(future, "socket_accept")
        }
    }

    // Collection of tokens associated with a UID.
    #[derive(Debug)]
    struct TokenCollection {
        // Sharing domain token. Allocated lazily on first use.
        sharing_domain_token: OnceLock<zx::Event>,
    }

    impl TokenCollection {
        fn new() -> Arc<Self> {
            Arc::new(Self { sharing_domain_token: OnceLock::new() })
        }

        /// Returns the number of handles for the tokens held beside this collection itself.
        fn get_handles_ref_count(&self) -> u32 {
            self.sharing_domain_token
                .get()
                .map(|token| {
                    token.count_info().expect("ZX_INFO_HANDLE_COUNT query failed").handle_count - 1
                })
                .unwrap_or(0)
        }
    }

    impl TokenCollection {
        fn get_sharing_domain_token(&self) -> zx::NullableHandle {
            self.sharing_domain_token
                .get_or_init(|| zx::Event::create())
                .duplicate_handle(zx::Rights::TRANSFER)
                .expect("Failed to duplicate handle")
                .into()
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum UidEntryState {
        // The entry is unused and can be removed.
        Unused,

        // The entry is being referenced by sockets.
        Used,

        // The entry is not referenced by sockets, but the sharing domains socket
        // is still referenced by netstack.
        Linger,
    }

    // Information stored for each UID in `SocketTokensStore`. Each entry is kept
    // only as long as there may be sockets associated with this UID.
    #[derive(Derivative)]
    #[derivative(Clone(bound = ""))]
    struct UidEntry<H: SocketTokenStoreHost> {
        tokens: Arc<TokenCollection>,

        // Weak reference to the resolver associated with this UID.
        weak_resolver: Weak<SocketTokenResolver<H>>,

        // Whether the cleanup task is running.
        cleanup_task_running: bool,
    }

    // SAFETY: UidEntry does not have Drop side effects.
    unsafe impl<H: SocketTokenStoreHost> RcuDroppable for UidEntry<H> {}

    impl<H: SocketTokenStoreHost> UidEntry<H> {
        fn new() -> Self {
            Self {
                tokens: TokenCollection::new(),
                weak_resolver: Weak::new(),
                cleanup_task_running: false,
            }
        }

        fn get_state(&self) -> UidEntryState {
            match (self.tokens.get_handles_ref_count(), self.weak_resolver.strong_count()) {
                (0, 0) => UidEntryState::Unused,
                (_, 0) => UidEntryState::Linger,
                _ => UidEntryState::Used,
            }
        }
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub struct SocketTokensStore<H: SocketTokenStoreHost = Kernel> {
        map: RcuHashMap<uid_t, UidEntry<H>>,
    }

    /// Delay between cleanup attempts.
    const CLEANUP_RETRY_DELAY: zx::MonotonicDuration = zx::MonotonicDuration::from_minutes(1);

    impl<H: SocketTokenStoreHost> SocketTokensStore<H> {
        pub(super) fn get_token_resolver(
            &self,
            host: &Arc<H>,
            uid: uid_t,
        ) -> Arc<SocketTokenResolver<H>> {
            let mut guard = self.map.lock();
            let mut entry = guard.entry(uid).or_insert_with(|| UidEntry::new());

            match entry.get().weak_resolver.upgrade() {
                Some(resolver) => resolver,
                None => {
                    let resolver = SocketTokenResolver::new(entry.get().tokens, host, uid);
                    let mut new_entry = entry.get();
                    new_entry.weak_resolver = Arc::downgrade(&resolver);
                    entry.insert(new_entry);
                    resolver
                }
            }
        }

        fn on_resolver_dropped(&self, host: &Arc<H>, uid: uid_t) {
            {
                let mut guard = self.map.lock();
                let Entry::Occupied(mut entry) = guard.entry(uid) else {
                    // The entry may be missing if another thread has created and removed another
                    // resolver for this UID.
                    return;
                };
                if entry.get().cleanup_task_running {
                    return;
                }
                match entry.get().get_state() {
                    UidEntryState::Unused => {
                        entry.remove();
                        return;
                    }
                    UidEntryState::Used => return,
                    UidEntryState::Linger => (),
                }

                let mut new_entry = entry.get();
                new_entry.cleanup_task_running = true;
                entry.insert(new_entry);
            }

            // Start cleanup task.
            let weak_host = Arc::downgrade(host);
            host.spawn_future(async move || {
                loop {
                    // Wait for a bit and then retry cleanup.
                    fasync::Timer::new(fasync::MonotonicInstant::after(CLEANUP_RETRY_DELAY)).await;

                    let Some(host) = weak_host.upgrade() else {
                        return;
                    };
                    let mut guard = host.get_socket_tokens_store().map.lock();
                    let Entry::Occupied(mut entry) = guard.entry(uid) else {
                        return;
                    };
                    match entry.get().get_state() {
                        UidEntryState::Unused => {
                            // We can remove the entry now.
                            entry.remove();
                            return;
                        }
                        UidEntryState::Used => {
                            // Quit cleanup task. It will be restarted later in
                            // `on_resolver_dropped()`.
                            let mut new_entry = entry.get();
                            new_entry.cleanup_task_running = false;
                            entry.insert(new_entry);
                            return;
                        }
                        UidEntryState::Linger => (),
                    }
                }
            });
        }
    }

    // Resolver for socket tokens. This type essentially acts as a proxy for
    // `TokenCollection` that also notifies `SocketTokensStore` when it is dropped.
    pub struct SocketTokenResolver<H: SocketTokenStoreHost> {
        tokens: Arc<TokenCollection>,

        // Used in `Drop` implementation to cleanup the entry in
        // `SocketTokensStore`.
        host: Weak<H>,
        uid: uid_t,
    }

    impl<H: SocketTokenStoreHost> SocketTokenResolver<H> {
        fn new(tokens: Arc<TokenCollection>, host: &Arc<H>, uid: uid_t) -> Arc<Self> {
            Arc::new(Self { tokens, host: Arc::downgrade(host), uid })
        }

        pub fn get_sharing_domain_token(&self) -> zx::NullableHandle {
            self.tokens.get_sharing_domain_token()
        }
    }

    impl<H: SocketTokenStoreHost> Drop for SocketTokenResolver<H> {
        fn drop(&mut self) {
            if let Some(host) = self.host.upgrade() {
                host.get_socket_tokens_store().on_resolver_dropped(&host, self.uid);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fuchsia_async::TestExecutor;
        use std::pin::pin;
        use test_case::test_matrix;
        use zx::MonotonicDuration;

        struct TestSocketTokenStoreHost {
            socket_tokens_store: SocketTokensStore<TestSocketTokenStoreHost>,
        }
        impl TestSocketTokenStoreHost {
            fn new() -> Arc<Self> {
                Arc::new(Self { socket_tokens_store: SocketTokensStore::default() })
            }
        }

        impl SocketTokenStoreHost for TestSocketTokenStoreHost {
            fn get_socket_tokens_store(&self) -> &SocketTokensStore<Self> {
                &self.socket_tokens_store
            }
            fn spawn_future(&self, future: impl AsyncFnOnce() -> () + Send + 'static) {
                fasync::Task::spawn(async move { fasync::Task::local(future()).await }).detach();
            }
        }

        fn advance_time(executor: &mut TestExecutor, d: MonotonicDuration) {
            let r = executor.run_until_stalled(&mut pin!(TestExecutor::advance_to(
                fasync::MonotonicInstant::after(d)
            )));
            assert!(r.is_ready());
        }

        const UID: uid_t = 100;

        #[::fuchsia::test]
        fn test_socket_tokens_store_base() {
            let host = TestSocketTokenStoreHost::new();
            let store = &host.socket_tokens_store;
            let token_resolver = store.get_token_resolver(&host, UID);
            assert!(store.map.lock().contains_key(&UID));
            drop(token_resolver);
            assert!(!store.map.lock().contains_key(&UID));
        }

        #[::fuchsia::test]
        fn test_socket_tokens_store_drop_handle_first() {
            let host = TestSocketTokenStoreHost::new();
            let store = &host.socket_tokens_store;
            let token_resolver = store.get_token_resolver(&host, UID);
            assert!(store.map.lock().contains_key(&UID));

            let token = token_resolver.get_sharing_domain_token();
            drop(token);
            drop(token_resolver);

            assert!(!store.map.lock().contains_key(&UID));
        }

        #[::fuchsia::test]
        fn test_socket_tokens_store_linger() {
            let mut executor = TestExecutor::new_with_fake_time();
            let host = TestSocketTokenStoreHost::new();
            let store = &host.socket_tokens_store;
            let token_resolver = store.get_token_resolver(&host, UID);
            let token = token_resolver.get_sharing_domain_token();
            assert!(store.map.lock().contains_key(&UID));
            drop(token_resolver);

            // The entry should not be dropped since we still hold the token
            assert!(store.map.lock().contains_key(&UID));
            advance_time(&mut executor, CLEANUP_RETRY_DELAY * 2);
            assert!(store.map.lock().contains_key(&UID));

            // The entry should be dropped shortly after the token is dropped.
            drop(token);
            advance_time(&mut executor, CLEANUP_RETRY_DELAY);
            assert!(!store.map.lock().contains_key(&UID));
        }

        #[test_matrix(
            [CLEANUP_RETRY_DELAY / 2,
            CLEANUP_RETRY_DELAY,
            CLEANUP_RETRY_DELAY * 3 / 2],
            [CLEANUP_RETRY_DELAY / 2,
            CLEANUP_RETRY_DELAY,
            CLEANUP_RETRY_DELAY * 3 / 2],
            [true, false]
        )]
        #[::fuchsia::test]
        fn test_socket_tokens_store_recreate(
            delay1: MonotonicDuration,
            delay2: MonotonicDuration,
            drop_tokens_first: bool,
        ) {
            let mut executor = TestExecutor::new_with_fake_time();
            let host = TestSocketTokenStoreHost::new();
            let store = &host.socket_tokens_store;
            let token_resolver = store.get_token_resolver(&host, UID);
            let token1 = token_resolver.get_sharing_domain_token();
            drop(token_resolver);

            // The entry should not be dropped since we still hold the token
            advance_time(&mut executor, delay1);
            assert!(store.map.lock().contains_key(&UID));

            // Create another resolver. It should reuse the same token.
            let token_resolver = store.get_token_resolver(&host, UID);
            let token2 = token_resolver.get_sharing_domain_token();
            assert!(token1.koid() == token2.koid());

            // Token should not be dropped while we have a TokenResolver.
            advance_time(&mut executor, delay2);
            assert!(store.map.lock().contains_key(&UID));

            if drop_tokens_first {
                drop(token1);
                drop(token2);
                drop(token_resolver);
            } else {
                drop(token_resolver);
                drop(token1);
                drop(token2);
            }

            // The timer is expected to be rescheduled if it ran between `delay1` and
            // `delay1 + delay2`.
            let timer_rescheduled = (delay1.into_seconds() / CLEANUP_RETRY_DELAY.into_seconds())
                != ((delay1 + delay2).into_seconds() / CLEANUP_RETRY_DELAY.into_seconds());

            if timer_rescheduled && drop_tokens_first {
                // The entry should be dropped since we dropped the tokens first.
                assert!(!store.map.lock().contains_key(&UID));
            } else {
                let expected_cleanup_delay = if timer_rescheduled {
                    CLEANUP_RETRY_DELAY
                } else {
                    CLEANUP_RETRY_DELAY
                        - MonotonicDuration::from_seconds(
                            (delay1 + delay2).into_seconds() % CLEANUP_RETRY_DELAY.into_seconds(),
                        )
                };

                // The tokens should be dropped exactly after `expected_cleanup_delay`.
                let one_second = MonotonicDuration::from_seconds(1);
                advance_time(&mut executor, expected_cleanup_delay - one_second);
                assert!(store.map.lock().contains_key(&UID));
                advance_time(&mut executor, one_second);
                assert!(!store.map.lock().contains_key(&UID));
            }
        }
    }
}

// Check that values that are passed to and from ZXIO have the same meaning.
const_assert_eq!(syncio::zxio::AF_UNSPEC, uapi::AF_UNSPEC as u32);
const_assert_eq!(syncio::zxio::AF_UNIX, uapi::AF_UNIX as u32);
const_assert_eq!(syncio::zxio::AF_INET, uapi::AF_INET as u32);
const_assert_eq!(syncio::zxio::AF_INET6, uapi::AF_INET6 as u32);
const_assert_eq!(syncio::zxio::AF_NETLINK, uapi::AF_NETLINK as u32);
const_assert_eq!(syncio::zxio::AF_PACKET, uapi::AF_PACKET as u32);
const_assert_eq!(syncio::zxio::AF_VSOCK, uapi::AF_VSOCK as u32);

const_assert_eq!(syncio::zxio::SO_DEBUG, uapi::SO_DEBUG);
const_assert_eq!(syncio::zxio::SO_REUSEADDR, uapi::SO_REUSEADDR);
const_assert_eq!(syncio::zxio::SO_TYPE, uapi::SO_TYPE);
const_assert_eq!(syncio::zxio::SO_ERROR, uapi::SO_ERROR);
const_assert_eq!(syncio::zxio::SO_DONTROUTE, uapi::SO_DONTROUTE);
const_assert_eq!(syncio::zxio::SO_BROADCAST, uapi::SO_BROADCAST);
const_assert_eq!(syncio::zxio::SO_SNDBUF, uapi::SO_SNDBUF);
const_assert_eq!(syncio::zxio::SO_RCVBUF, uapi::SO_RCVBUF);
const_assert_eq!(syncio::zxio::SO_KEEPALIVE, uapi::SO_KEEPALIVE);
const_assert_eq!(syncio::zxio::SO_OOBINLINE, uapi::SO_OOBINLINE);
const_assert_eq!(syncio::zxio::SO_NO_CHECK, uapi::SO_NO_CHECK);
const_assert_eq!(syncio::zxio::SO_PRIORITY, uapi::SO_PRIORITY);
const_assert_eq!(syncio::zxio::SO_LINGER, uapi::SO_LINGER);
const_assert_eq!(syncio::zxio::SO_BSDCOMPAT, uapi::SO_BSDCOMPAT);
const_assert_eq!(syncio::zxio::SO_REUSEPORT, uapi::SO_REUSEPORT);
const_assert_eq!(syncio::zxio::SO_PASSCRED, uapi::SO_PASSCRED);
const_assert_eq!(syncio::zxio::SO_PEERCRED, uapi::SO_PEERCRED);
const_assert_eq!(syncio::zxio::SO_RCVLOWAT, uapi::SO_RCVLOWAT);
const_assert_eq!(syncio::zxio::SO_SNDLOWAT, uapi::SO_SNDLOWAT);
const_assert_eq!(syncio::zxio::SO_ACCEPTCONN, uapi::SO_ACCEPTCONN);
const_assert_eq!(syncio::zxio::SO_PEERSEC, uapi::SO_PEERSEC);
const_assert_eq!(syncio::zxio::SO_SNDBUFFORCE, uapi::SO_SNDBUFFORCE);
const_assert_eq!(syncio::zxio::SO_RCVBUFFORCE, uapi::SO_RCVBUFFORCE);
const_assert_eq!(syncio::zxio::SO_PROTOCOL, uapi::SO_PROTOCOL);
const_assert_eq!(syncio::zxio::SO_DOMAIN, uapi::SO_DOMAIN);
const_assert_eq!(syncio::zxio::SO_RCVTIMEO, uapi::SO_RCVTIMEO);
const_assert_eq!(syncio::zxio::SO_SNDTIMEO, uapi::SO_SNDTIMEO);
const_assert_eq!(syncio::zxio::SO_TIMESTAMP, uapi::SO_TIMESTAMP);
const_assert_eq!(syncio::zxio::SO_TIMESTAMPNS, uapi::SO_TIMESTAMPNS);
const_assert_eq!(syncio::zxio::SO_TIMESTAMPING, uapi::SO_TIMESTAMPING);
const_assert_eq!(syncio::zxio::SO_SECURITY_AUTHENTICATION, uapi::SO_SECURITY_AUTHENTICATION);
const_assert_eq!(
    syncio::zxio::SO_SECURITY_ENCRYPTION_TRANSPORT,
    uapi::SO_SECURITY_ENCRYPTION_TRANSPORT
);
const_assert_eq!(
    syncio::zxio::SO_SECURITY_ENCRYPTION_NETWORK,
    uapi::SO_SECURITY_ENCRYPTION_NETWORK
);
const_assert_eq!(syncio::zxio::SO_BINDTODEVICE, uapi::SO_BINDTODEVICE);
const_assert_eq!(syncio::zxio::SO_ATTACH_FILTER, uapi::SO_ATTACH_FILTER);
const_assert_eq!(syncio::zxio::SO_DETACH_FILTER, uapi::SO_DETACH_FILTER);
const_assert_eq!(syncio::zxio::SO_GET_FILTER, uapi::SO_GET_FILTER);
const_assert_eq!(syncio::zxio::SO_PEERNAME, uapi::SO_PEERNAME);
const_assert_eq!(syncio::zxio::SO_PASSSEC, uapi::SO_PASSSEC);
const_assert_eq!(syncio::zxio::SO_MARK, uapi::SO_MARK);
const_assert_eq!(syncio::zxio::SO_RXQ_OVFL, uapi::SO_RXQ_OVFL);
const_assert_eq!(syncio::zxio::SO_WIFI_STATUS, uapi::SO_WIFI_STATUS);
const_assert_eq!(syncio::zxio::SO_PEEK_OFF, uapi::SO_PEEK_OFF);
const_assert_eq!(syncio::zxio::SO_NOFCS, uapi::SO_NOFCS);
const_assert_eq!(syncio::zxio::SO_LOCK_FILTER, uapi::SO_LOCK_FILTER);
const_assert_eq!(syncio::zxio::SO_SELECT_ERR_QUEUE, uapi::SO_SELECT_ERR_QUEUE);
const_assert_eq!(syncio::zxio::SO_BUSY_POLL, uapi::SO_BUSY_POLL);
const_assert_eq!(syncio::zxio::SO_MAX_PACING_RATE, uapi::SO_MAX_PACING_RATE);
const_assert_eq!(syncio::zxio::SO_BPF_EXTENSIONS, uapi::SO_BPF_EXTENSIONS);
const_assert_eq!(syncio::zxio::SO_INCOMING_CPU, uapi::SO_INCOMING_CPU);
const_assert_eq!(syncio::zxio::SO_ATTACH_BPF, uapi::SO_ATTACH_BPF);
const_assert_eq!(syncio::zxio::SO_DETACH_BPF, uapi::SO_DETACH_BPF);
const_assert_eq!(syncio::zxio::SO_ATTACH_REUSEPORT_CBPF, uapi::SO_ATTACH_REUSEPORT_CBPF);
const_assert_eq!(syncio::zxio::SO_ATTACH_REUSEPORT_EBPF, uapi::SO_ATTACH_REUSEPORT_EBPF);
const_assert_eq!(syncio::zxio::SO_CNX_ADVICE, uapi::SO_CNX_ADVICE);
const_assert_eq!(syncio::zxio::SO_MEMINFO, uapi::SO_MEMINFO);
const_assert_eq!(syncio::zxio::SO_INCOMING_NAPI_ID, uapi::SO_INCOMING_NAPI_ID);
const_assert_eq!(syncio::zxio::SO_COOKIE, uapi::SO_COOKIE);
const_assert_eq!(syncio::zxio::SO_PEERGROUPS, uapi::SO_PEERGROUPS);
const_assert_eq!(syncio::zxio::SO_ZEROCOPY, uapi::SO_ZEROCOPY);
const_assert_eq!(syncio::zxio::SO_TXTIME, uapi::SO_TXTIME);
const_assert_eq!(syncio::zxio::SO_BINDTOIFINDEX, uapi::SO_BINDTOIFINDEX);
const_assert_eq!(syncio::zxio::SO_DETACH_REUSEPORT_BPF, uapi::SO_DETACH_REUSEPORT_BPF);
const_assert_eq!(syncio::zxio::SO_ORIGINAL_DST, uapi::SO_ORIGINAL_DST);

const_assert_eq!(syncio::zxio::MSG_WAITALL, uapi::MSG_WAITALL);
const_assert_eq!(syncio::zxio::MSG_PEEK, uapi::MSG_PEEK);
const_assert_eq!(syncio::zxio::MSG_DONTROUTE, uapi::MSG_DONTROUTE);
const_assert_eq!(syncio::zxio::MSG_CTRUNC, uapi::MSG_CTRUNC);
const_assert_eq!(syncio::zxio::MSG_PROXY, uapi::MSG_PROXY);
const_assert_eq!(syncio::zxio::MSG_TRUNC, uapi::MSG_TRUNC);
const_assert_eq!(syncio::zxio::MSG_DONTWAIT, uapi::MSG_DONTWAIT);
const_assert_eq!(syncio::zxio::MSG_EOR, uapi::MSG_EOR);
const_assert_eq!(syncio::zxio::MSG_WAITALL, uapi::MSG_WAITALL);
const_assert_eq!(syncio::zxio::MSG_FIN, uapi::MSG_FIN);
const_assert_eq!(syncio::zxio::MSG_SYN, uapi::MSG_SYN);
const_assert_eq!(syncio::zxio::MSG_CONFIRM, uapi::MSG_CONFIRM);
const_assert_eq!(syncio::zxio::MSG_RST, uapi::MSG_RST);
const_assert_eq!(syncio::zxio::MSG_ERRQUEUE, uapi::MSG_ERRQUEUE);
const_assert_eq!(syncio::zxio::MSG_NOSIGNAL, uapi::MSG_NOSIGNAL);
const_assert_eq!(syncio::zxio::MSG_MORE, uapi::MSG_MORE);
const_assert_eq!(syncio::zxio::MSG_WAITFORONE, uapi::MSG_WAITFORONE);
const_assert_eq!(syncio::zxio::MSG_BATCH, uapi::MSG_BATCH);
const_assert_eq!(syncio::zxio::MSG_FASTOPEN, uapi::MSG_FASTOPEN);
const_assert_eq!(syncio::zxio::MSG_CMSG_CLOEXEC, uapi::MSG_CMSG_CLOEXEC);

const_assert_eq!(syncio::zxio::IP_TOS, uapi::IP_TOS);
const_assert_eq!(syncio::zxio::IP_TTL, uapi::IP_TTL);
const_assert_eq!(syncio::zxio::IP_HDRINCL, uapi::IP_HDRINCL);
const_assert_eq!(syncio::zxio::IP_OPTIONS, uapi::IP_OPTIONS);
const_assert_eq!(syncio::zxio::IP_ROUTER_ALERT, uapi::IP_ROUTER_ALERT);
const_assert_eq!(syncio::zxio::IP_RECVOPTS, uapi::IP_RECVOPTS);
const_assert_eq!(syncio::zxio::IP_RETOPTS, uapi::IP_RETOPTS);
const_assert_eq!(syncio::zxio::IP_PKTINFO, uapi::IP_PKTINFO);
const_assert_eq!(syncio::zxio::IP_PKTOPTIONS, uapi::IP_PKTOPTIONS);
const_assert_eq!(syncio::zxio::IP_MTU_DISCOVER, uapi::IP_MTU_DISCOVER);
const_assert_eq!(syncio::zxio::IP_RECVERR, uapi::IP_RECVERR);
const_assert_eq!(syncio::zxio::IP_RECVTTL, uapi::IP_RECVTTL);
const_assert_eq!(syncio::zxio::IP_RECVTOS, uapi::IP_RECVTOS);
const_assert_eq!(syncio::zxio::IP_MTU, uapi::IP_MTU);
const_assert_eq!(syncio::zxio::IP_FREEBIND, uapi::IP_FREEBIND);
const_assert_eq!(syncio::zxio::IP_IPSEC_POLICY, uapi::IP_IPSEC_POLICY);
const_assert_eq!(syncio::zxio::IP_XFRM_POLICY, uapi::IP_XFRM_POLICY);
const_assert_eq!(syncio::zxio::IP_PASSSEC, uapi::IP_PASSSEC);
const_assert_eq!(syncio::zxio::IP_TRANSPARENT, uapi::IP_TRANSPARENT);
const_assert_eq!(syncio::zxio::IP_ORIGDSTADDR, uapi::IP_ORIGDSTADDR);
const_assert_eq!(syncio::zxio::IP_RECVORIGDSTADDR, uapi::IP_RECVORIGDSTADDR);
const_assert_eq!(syncio::zxio::IP_MINTTL, uapi::IP_MINTTL);
const_assert_eq!(syncio::zxio::IP_NODEFRAG, uapi::IP_NODEFRAG);
const_assert_eq!(syncio::zxio::IP_CHECKSUM, uapi::IP_CHECKSUM);
const_assert_eq!(syncio::zxio::IP_BIND_ADDRESS_NO_PORT, uapi::IP_BIND_ADDRESS_NO_PORT);
const_assert_eq!(syncio::zxio::IP_MULTICAST_IF, uapi::IP_MULTICAST_IF);
const_assert_eq!(syncio::zxio::IP_MULTICAST_TTL, uapi::IP_MULTICAST_TTL);
const_assert_eq!(syncio::zxio::IP_MULTICAST_LOOP, uapi::IP_MULTICAST_LOOP);
const_assert_eq!(syncio::zxio::IP_ADD_MEMBERSHIP, uapi::IP_ADD_MEMBERSHIP);
const_assert_eq!(syncio::zxio::IP_DROP_MEMBERSHIP, uapi::IP_DROP_MEMBERSHIP);
const_assert_eq!(syncio::zxio::IP_UNBLOCK_SOURCE, uapi::IP_UNBLOCK_SOURCE);
const_assert_eq!(syncio::zxio::IP_BLOCK_SOURCE, uapi::IP_BLOCK_SOURCE);
const_assert_eq!(syncio::zxio::IP_ADD_SOURCE_MEMBERSHIP, uapi::IP_ADD_SOURCE_MEMBERSHIP);
const_assert_eq!(syncio::zxio::IP_DROP_SOURCE_MEMBERSHIP, uapi::IP_DROP_SOURCE_MEMBERSHIP);
const_assert_eq!(syncio::zxio::IP_MSFILTER, uapi::IP_MSFILTER);
const_assert_eq!(syncio::zxio::IP_MULTICAST_ALL, uapi::IP_MULTICAST_ALL);
const_assert_eq!(syncio::zxio::IP_UNICAST_IF, uapi::IP_UNICAST_IF);
const_assert_eq!(syncio::zxio::IP_RECVRETOPTS, uapi::IP_RECVRETOPTS);
const_assert_eq!(syncio::zxio::IP_PMTUDISC_DONT, uapi::IP_PMTUDISC_DONT);
const_assert_eq!(syncio::zxio::IP_PMTUDISC_WANT, uapi::IP_PMTUDISC_WANT);
const_assert_eq!(syncio::zxio::IP_PMTUDISC_DO, uapi::IP_PMTUDISC_DO);
const_assert_eq!(syncio::zxio::IP_PMTUDISC_PROBE, uapi::IP_PMTUDISC_PROBE);
const_assert_eq!(syncio::zxio::IP_PMTUDISC_INTERFACE, uapi::IP_PMTUDISC_INTERFACE);
const_assert_eq!(syncio::zxio::IP_PMTUDISC_OMIT, uapi::IP_PMTUDISC_OMIT);
const_assert_eq!(syncio::zxio::IP_DEFAULT_MULTICAST_TTL, uapi::IP_DEFAULT_MULTICAST_TTL);
const_assert_eq!(syncio::zxio::IP_DEFAULT_MULTICAST_LOOP, uapi::IP_DEFAULT_MULTICAST_LOOP);

const_assert_eq!(syncio::zxio::IPV6_ADDRFORM, uapi::IPV6_ADDRFORM);
const_assert_eq!(syncio::zxio::IPV6_2292PKTINFO, uapi::IPV6_2292PKTINFO);
const_assert_eq!(syncio::zxio::IPV6_2292HOPOPTS, uapi::IPV6_2292HOPOPTS);
const_assert_eq!(syncio::zxio::IPV6_2292DSTOPTS, uapi::IPV6_2292DSTOPTS);
const_assert_eq!(syncio::zxio::IPV6_2292RTHDR, uapi::IPV6_2292RTHDR);
const_assert_eq!(syncio::zxio::IPV6_2292PKTOPTIONS, uapi::IPV6_2292PKTOPTIONS);
const_assert_eq!(syncio::zxio::IPV6_CHECKSUM, uapi::IPV6_CHECKSUM);
const_assert_eq!(syncio::zxio::IPV6_2292HOPLIMIT, uapi::IPV6_2292HOPLIMIT);
const_assert_eq!(syncio::zxio::IPV6_NEXTHOP, uapi::IPV6_NEXTHOP);
const_assert_eq!(syncio::zxio::IPV6_AUTHHDR, uapi::IPV6_AUTHHDR);
const_assert_eq!(syncio::zxio::IPV6_UNICAST_HOPS, uapi::IPV6_UNICAST_HOPS);
const_assert_eq!(syncio::zxio::IPV6_MULTICAST_IF, uapi::IPV6_MULTICAST_IF);
const_assert_eq!(syncio::zxio::IPV6_MULTICAST_HOPS, uapi::IPV6_MULTICAST_HOPS);
const_assert_eq!(syncio::zxio::IPV6_MULTICAST_LOOP, uapi::IPV6_MULTICAST_LOOP);
const_assert_eq!(syncio::zxio::IPV6_ROUTER_ALERT, uapi::IPV6_ROUTER_ALERT);
const_assert_eq!(syncio::zxio::IPV6_MTU_DISCOVER, uapi::IPV6_MTU_DISCOVER);
const_assert_eq!(syncio::zxio::IPV6_MTU, uapi::IPV6_MTU);
const_assert_eq!(syncio::zxio::IPV6_RECVERR, uapi::IPV6_RECVERR);
const_assert_eq!(syncio::zxio::IPV6_V6ONLY, uapi::IPV6_V6ONLY);
const_assert_eq!(syncio::zxio::IPV6_JOIN_ANYCAST, uapi::IPV6_JOIN_ANYCAST);
const_assert_eq!(syncio::zxio::IPV6_LEAVE_ANYCAST, uapi::IPV6_LEAVE_ANYCAST);
const_assert_eq!(syncio::zxio::IPV6_IPSEC_POLICY, uapi::IPV6_IPSEC_POLICY);
const_assert_eq!(syncio::zxio::IPV6_XFRM_POLICY, uapi::IPV6_XFRM_POLICY);
const_assert_eq!(syncio::zxio::IPV6_HDRINCL, uapi::IPV6_HDRINCL);
const_assert_eq!(syncio::zxio::IPV6_RECVPKTINFO, uapi::IPV6_RECVPKTINFO);
const_assert_eq!(syncio::zxio::IPV6_PKTINFO, uapi::IPV6_PKTINFO);
const_assert_eq!(syncio::zxio::IPV6_RECVHOPLIMIT, uapi::IPV6_RECVHOPLIMIT);
const_assert_eq!(syncio::zxio::IPV6_HOPLIMIT, uapi::IPV6_HOPLIMIT);
const_assert_eq!(syncio::zxio::IPV6_RECVHOPOPTS, uapi::IPV6_RECVHOPOPTS);
const_assert_eq!(syncio::zxio::IPV6_HOPOPTS, uapi::IPV6_HOPOPTS);
const_assert_eq!(syncio::zxio::IPV6_RTHDRDSTOPTS, uapi::IPV6_RTHDRDSTOPTS);
const_assert_eq!(syncio::zxio::IPV6_RECVRTHDR, uapi::IPV6_RECVRTHDR);
const_assert_eq!(syncio::zxio::IPV6_RTHDR, uapi::IPV6_RTHDR);
const_assert_eq!(syncio::zxio::IPV6_RECVDSTOPTS, uapi::IPV6_RECVDSTOPTS);
const_assert_eq!(syncio::zxio::IPV6_DSTOPTS, uapi::IPV6_DSTOPTS);
const_assert_eq!(syncio::zxio::IPV6_RECVPATHMTU, uapi::IPV6_RECVPATHMTU);
const_assert_eq!(syncio::zxio::IPV6_PATHMTU, uapi::IPV6_PATHMTU);
const_assert_eq!(syncio::zxio::IPV6_DONTFRAG, uapi::IPV6_DONTFRAG);
const_assert_eq!(syncio::zxio::IPV6_RECVTCLASS, uapi::IPV6_RECVTCLASS);
const_assert_eq!(syncio::zxio::IPV6_TCLASS, uapi::IPV6_TCLASS);
const_assert_eq!(syncio::zxio::IPV6_AUTOFLOWLABEL, uapi::IPV6_AUTOFLOWLABEL);
const_assert_eq!(syncio::zxio::IPV6_ADDR_PREFERENCES, uapi::IPV6_ADDR_PREFERENCES);
const_assert_eq!(syncio::zxio::IPV6_MINHOPCOUNT, uapi::IPV6_MINHOPCOUNT);
const_assert_eq!(syncio::zxio::IPV6_ORIGDSTADDR, uapi::IPV6_ORIGDSTADDR);
const_assert_eq!(syncio::zxio::IPV6_RECVORIGDSTADDR, uapi::IPV6_RECVORIGDSTADDR);
const_assert_eq!(syncio::zxio::IPV6_TRANSPARENT, uapi::IPV6_TRANSPARENT);
const_assert_eq!(syncio::zxio::IPV6_UNICAST_IF, uapi::IPV6_UNICAST_IF);
const_assert_eq!(syncio::zxio::IPV6_ADD_MEMBERSHIP, uapi::IPV6_ADD_MEMBERSHIP);
const_assert_eq!(syncio::zxio::IPV6_DROP_MEMBERSHIP, uapi::IPV6_DROP_MEMBERSHIP);
const_assert_eq!(syncio::zxio::IPV6_PMTUDISC_DONT, uapi::IPV6_PMTUDISC_DONT);
const_assert_eq!(syncio::zxio::IPV6_PMTUDISC_WANT, uapi::IPV6_PMTUDISC_WANT);
const_assert_eq!(syncio::zxio::IPV6_PMTUDISC_DO, uapi::IPV6_PMTUDISC_DO);
const_assert_eq!(syncio::zxio::IPV6_PMTUDISC_PROBE, uapi::IPV6_PMTUDISC_PROBE);
const_assert_eq!(syncio::zxio::IPV6_PMTUDISC_INTERFACE, uapi::IPV6_PMTUDISC_INTERFACE);
const_assert_eq!(syncio::zxio::IPV6_PMTUDISC_OMIT, uapi::IPV6_PMTUDISC_OMIT);
const_assert_eq!(syncio::zxio::IPV6_PREFER_SRC_TMP, uapi::IPV6_PREFER_SRC_TMP);
const_assert_eq!(syncio::zxio::IPV6_PREFER_SRC_PUBLIC, uapi::IPV6_PREFER_SRC_PUBLIC);
const_assert_eq!(
    syncio::zxio::IPV6_PREFER_SRC_PUBTMP_DEFAULT,
    uapi::IPV6_PREFER_SRC_PUBTMP_DEFAULT
);
const_assert_eq!(syncio::zxio::IPV6_PREFER_SRC_COA, uapi::IPV6_PREFER_SRC_COA);
const_assert_eq!(syncio::zxio::IPV6_PREFER_SRC_HOME, uapi::IPV6_PREFER_SRC_HOME);
const_assert_eq!(syncio::zxio::IPV6_PREFER_SRC_CGA, uapi::IPV6_PREFER_SRC_CGA);
const_assert_eq!(syncio::zxio::IPV6_PREFER_SRC_NONCGA, uapi::IPV6_PREFER_SRC_NONCGA);
