// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bpf::attachments::SetSockOptProgramResult;
use crate::mm::{IOVecPtr, MemoryAccessor, MemoryAccessorExt};
use crate::security;
use crate::syscalls::time::TimeSpecPtr;
use crate::task::{CurrentTask, IpTables, Task, WaitCallback, Waiter};
use crate::vfs::buffers::{
    AncillaryData, ControlMsg, UserBuffersInputBuffer, UserBuffersOutputBuffer,
};
use crate::vfs::socket::{
    SA_FAMILY_SIZE, SA_STORAGE_SIZE, Socket, SocketAddress, SocketDomain, SocketFile,
    SocketMessageFlags, SocketPeer, SocketProtocol, SocketShutdownFlags, SocketType, UnixSocket,
    resolve_unix_socket_address,
};
use crate::vfs::{FdFlags, FdNumber, FileHandle, FsString, LookupContext};
use starnix_logging::{log_trace, track_stub};
use starnix_types::augmented::Augmented;
use starnix_types::time::duration_from_timespec;
use starnix_types::user_buffer::{UserBuffer, UserBuffers};
use starnix_uapi::auth::CAP_NET_BIND_SERVICE;
use starnix_uapi::errors::{EEXIST, EINPROGRESS, Errno};
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::math::round_up_to_increment;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{
    ArchSpecific, MappingMultiArchUserRef, MultiArchUserRef, UserAddress, UserRef,
};
use starnix_uapi::user_value::UserValue;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    MSG_CTRUNC, MSG_DONTWAIT, MSG_TRUNC, MSG_WAITFORONE, SHUT_RD, SHUT_RDWR, SHUT_WR, SOCK_CLOEXEC,
    SOCK_NONBLOCK, UIO_MAXIOV, errno, error, socklen_t, uapi,
};
use std::ops::DerefMut;

uapi::check_arch_independent_layout! {
    socklen_t {}
}

/// A `msghdr` can be augmented with a `UserBuffer`. In that case, the `UserBuffer` is used for
/// the I/O, instead of the `iovec` fields from the `msghdr`.
pub type WithAlternateBuffer<T> = Augmented<T, UserBuffer>;
pub type MsgHdrPtr = MappingMultiArchUserRef<MsgHdr, uapi::msghdr, uapi::arch32::msghdr>;

#[derive(Debug, Clone)]
pub struct MsgHdr {
    pub name: UserAddress,
    pub name_len: socklen_t,
    pub iov: IOVecPtr,
    pub iovlen: UserValue<usize>,
    pub control: UserAddress,
    pub control_len: usize,
    pub flags: u32,
}

/// A reference to a `msghdr`.
///
/// This enum is used to abstract over whether the `msghdr` is in user memory (and needs to be
/// read) or has been constructed in the kernel. This is used by `io_uring` to provide a buffer
/// for `recvmsg`.
#[derive(Debug, Clone)]
pub enum MsgHdrRef {
    Ptr(MsgHdrPtr),
    Value(WithAlternateBuffer<MsgHdr>),
}

impl From<MsgHdrPtr> for MsgHdrRef {
    fn from(ptr: MsgHdrPtr) -> Self {
        Self::Ptr(ptr)
    }
}

impl From<WithAlternateBuffer<MsgHdr>> for MsgHdrRef {
    fn from(value: WithAlternateBuffer<MsgHdr>) -> Self {
        Self::Value(value)
    }
}

pub type MMsgHdrPtr = MappingMultiArchUserRef<MMsgHdr, uapi::mmsghdr, uapi::arch32::mmsghdr>;

pub struct MMsgHdr {
    hdr: MsgHdr,
    len: usize,
}

uapi::arch_map_data! {
    BidiTryFrom<MsgHdr, msghdr> {
        name = msg_name;
        name_len = msg_namelen;
        iov = msg_iov;
        iovlen = msg_iovlen;
        control = msg_control;
        control_len = msg_controllen;
        flags = msg_flags;
    }

    BidiTryFrom<MMsgHdr, mmsghdr> {
        hdr = msg_hdr;
        len = msg_len;
    }
}

pub type CMsgHdrPtr = MultiArchUserRef<uapi::cmsghdr, uapi::arch32::cmsghdr>;

pub fn sys_socket(
    current_task: &CurrentTask,
    domain: u32,
    socket_type: u32,
    protocol: u32,
) -> Result<FdNumber, Errno> {
    let flags = socket_type & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let domain = parse_socket_domain(domain)?;
    let socket_type = parse_socket_type(domain, socket_type)?;
    // Should we use parse_socket_protocol here?
    let protocol = SocketProtocol::from_raw(protocol);
    let open_flags = socket_flags_to_open_flags(flags);
    let socket_file = SocketFile::new_socket(
        current_task,
        domain,
        socket_type,
        open_flags,
        protocol,
        /*kernel_private=*/ false,
    )?;

    let fd_flags = socket_flags_to_fd_flags(flags);
    let fd = current_task.add_file(socket_file, fd_flags)?;
    Ok(fd)
}

fn socket_flags_to_open_flags(flags: u32) -> OpenFlags {
    OpenFlags::RDWR
        | if flags & SOCK_NONBLOCK != 0 { OpenFlags::NONBLOCK } else { OpenFlags::empty() }
}

fn socket_flags_to_fd_flags(flags: u32) -> FdFlags {
    if flags & SOCK_CLOEXEC != 0 { FdFlags::CLOEXEC } else { FdFlags::empty() }
}

fn parse_socket_domain(domain: u32) -> Result<SocketDomain, Errno> {
    SocketDomain::from_raw(domain.try_into().map_err(|_| errno!(EAFNOSUPPORT))?).ok_or_else(|| {
        track_stub!(TODO("https://fxbug.dev/322875074"), "parse socket domain", domain);
        errno!(EAFNOSUPPORT)
    })
}

fn parse_socket_type(domain: SocketDomain, socket_type: u32) -> Result<SocketType, Errno> {
    let socket_type = SocketType::from_raw(socket_type & 0xf).ok_or_else(|| {
        track_stub!(TODO("https://fxbug.dev/322875418"), "parse socket type", socket_type);
        errno!(EINVAL)
    })?;
    // For AF_UNIX, SOCK_RAW sockets are treated as if they were SOCK_DGRAM.
    Ok(if domain == SocketDomain::Unix && socket_type == SocketType::Raw {
        SocketType::Datagram
    } else {
        socket_type
    })
}

fn parse_socket_protocol(
    domain: SocketDomain,
    socket_type: SocketType,
    protocol: u32,
) -> Result<SocketProtocol, Errno> {
    let protocol = SocketProtocol::from_raw(protocol);
    if domain == SocketDomain::Inet {
        match (socket_type, protocol) {
            (SocketType::Raw, _) => {
                // Should we have different behavior error when called by root?
                return error!(EPROTONOSUPPORT);
            }
            (SocketType::Datagram, SocketProtocol::UDP) => (),
            (SocketType::Datagram, _) => return error!(EPROTONOSUPPORT),
            (SocketType::Stream, SocketProtocol::TCP) => (),
            (SocketType::Stream, _) => return error!(EPROTONOSUPPORT),
            _ => (),
        }
    }
    Ok(protocol)
}

fn parse_socket_address(
    task: &Task,
    user_socket_address: UserAddress,
    user_address_length: usize,
) -> Result<SocketAddress, Errno> {
    if user_address_length < SA_FAMILY_SIZE || user_address_length > SA_STORAGE_SIZE {
        return error!(EINVAL);
    }

    let address = task.read_memory_to_vec(user_socket_address, user_address_length)?;

    SocketAddress::from_bytes(address)
}

fn maybe_parse_socket_address(
    task: &Task,
    user_socket_address: UserAddress,
    user_address_length: usize,
) -> Result<Option<SocketAddress>, Errno> {
    if user_address_length > i32::MAX as usize {
        return error!(EINVAL);
    }
    Ok(if user_socket_address.is_null() {
        None
    } else {
        Some(parse_socket_address(task, user_socket_address, user_address_length)?)
    })
}

// See "Autobind feature" section of https://man7.org/linux/man-pages/man7/unix.7.html
fn generate_autobind_address() -> FsString {
    let mut bytes = [0u8; 4];
    starnix_crypto::cprng_draw(&mut bytes);
    let value = u32::from_ne_bytes(bytes) & 0xFFFFF;
    format!("\0{value:05x}").into()
}

pub fn sys_bind(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_socket_address: UserAddress,
    user_address_length: usize,
) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;
    let address = parse_socket_address(current_task, user_socket_address, user_address_length)?;
    if !address.valid_for_domain(socket.domain) {
        if socket.domain == SocketDomain::Inet && address == SocketAddress::Unspecified {
            if let Ok(local_addr) = socket.getsockname() {
                if local_addr.maybe_inet_port().unwrap_or(0) != 0 {
                    return Ok(());
                }
            }
        }
        return match socket.domain {
            SocketDomain::Unix
            | SocketDomain::Vsock
            | SocketDomain::Inet6
            | SocketDomain::Netlink
            | SocketDomain::Key
            | SocketDomain::Packet
            | SocketDomain::Qipcrtr => error!(EINVAL),
            SocketDomain::Inet => error!(EAFNOSUPPORT),
        };
    }
    if let Some(port) = address.maybe_inet_port() {
        // See <https://man7.org/linux/man-pages/man7/ip.7.html>:
        //
        //   The port numbers below 1024 are called privileged ports (or
        //   sometimes: reserved ports).  Only a privileged process (on Linux:
        //   a process that has the CAP_NET_BIND_SERVICE capability in the
        //   user namespace governing its network namespace) may bind(2) to
        //   these sockets.
        if port != 0 && port < 1024 {
            security::check_task_capable(current_task, CAP_NET_BIND_SERVICE)
                .map_err(|_| errno!(EACCES))?;
        }
    }
    security::check_socket_bind_access(current_task, socket, &address)?;
    match address {
        SocketAddress::Unspecified => return error!(EINVAL),
        SocketAddress::Unix(mut name) => {
            if name.is_empty() {
                // If the name is empty, then we're supposed to generate an
                // autobind address, which is always abstract.
                name = generate_autobind_address();
            }
            // If there is a null byte at the start of the sun_path, then the
            // address is abstract.
            if name[0] == b'\0' {
                current_task.running_state().abstract_socket_namespace.bind(
                    current_task,
                    name,
                    socket,
                )?;
            } else {
                let mode = file.node().info().mode;
                let mode = current_task.fs().apply_umask(mode).with_type(FileMode::IFSOCK);
                let (parent, basename) = current_task.lookup_parent_at(
                    &mut LookupContext::default(),
                    FdNumber::AT_FDCWD,
                    name.as_ref(),
                )?;

                parent
                    .bind_socket(
                        current_task,
                        basename,
                        socket.clone(),
                        SocketAddress::Unix(name.clone()),
                        mode,
                    )
                    .map_err(|errno| if errno == EEXIST { errno!(EADDRINUSE) } else { errno })?;
            }
        }
        SocketAddress::Vsock { port, .. } => {
            current_task.running_state().abstract_vsock_namespace.bind(
                current_task,
                port,
                socket,
            )?;
        }
        SocketAddress::Inet(_)
        | SocketAddress::Inet6(_)
        | SocketAddress::Netlink(_)
        | SocketAddress::Packet(_)
        | SocketAddress::Qipcrtr(_) => socket.bind(current_task, address)?,
    }

    Ok(())
}

pub fn sys_listen(current_task: &CurrentTask, fd: FdNumber, backlog: i32) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;
    socket.listen(current_task, backlog)?;
    Ok(())
}

pub fn sys_accept(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_socket_address: UserAddress,
    user_address_length: UserRef<socklen_t>,
) -> Result<FdNumber, Errno> {
    sys_accept4(current_task, fd, user_socket_address, user_address_length, 0)
}

pub fn sys_accept4(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_socket_address: UserAddress,
    user_address_length: UserRef<socklen_t>,
    flags: u32,
) -> Result<FdNumber, Errno> {
    let file = current_task.files().get(fd)?;
    let listening_socket = Socket::get_from_file(&file)?;
    let accepted_socket =
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            listening_socket.accept(current_task)
        })?;

    if !user_socket_address.is_null() {
        let address_bytes = accepted_socket.getpeername()?.to_bytes();
        write_socket_address(
            current_task,
            user_socket_address,
            user_address_length,
            &address_bytes,
        )?;
    }

    let open_flags = socket_flags_to_open_flags(flags);
    let accepted_socket_file = SocketFile::from_socket(
        current_task,
        accepted_socket,
        open_flags,
        /* kernel_private= */ false,
    )?;
    let listening_socket = SocketFile::get_from_file(&file)?;
    let accepted_socket = SocketFile::get_from_file(&accepted_socket_file)?;
    security::socket_accept(current_task, listening_socket, accepted_socket)?;
    let fd_flags = if flags & SOCK_CLOEXEC != 0 { FdFlags::CLOEXEC } else { FdFlags::empty() };
    let accepted_fd = current_task.add_file(accepted_socket_file, fd_flags)?;
    Ok(accepted_fd)
}

pub fn sys_connect(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_socket_address: UserAddress,
    user_address_length: usize,
) -> Result<(), Errno> {
    let client = current_task.files().get(fd)?;
    let client = SocketFile::get_from_file(&client)?;
    let address = parse_socket_address(current_task, user_socket_address, user_address_length)?;
    let peer = match address {
        SocketAddress::Unspecified => SocketPeer::Address(address),
        SocketAddress::Unix(ref name) => {
            log_trace!("connect to unix socket named \"{name}\"");
            if name.is_empty() {
                return error!(ECONNREFUSED);
            }
            SocketPeer::Handle(resolve_unix_socket_address(current_task, name.as_ref())?)
        }
        // TODO(https://fxbug.dev/445433238): Connect not available for AF_VSOCK
        SocketAddress::Vsock { .. } => return error!(ENOSYS),
        SocketAddress::Inet(ref addr) | SocketAddress::Inet6(ref addr) => {
            log_trace!("connect to inet socket named {:?}", addr);
            SocketPeer::Address(address)
        }
        SocketAddress::Netlink(_) => SocketPeer::Address(address),
        SocketAddress::Packet(ref addr) => {
            log_trace!("connect to packet socket named {:?}", addr);
            SocketPeer::Address(address)
        }
        SocketAddress::Qipcrtr(ref addr) => {
            log_trace!("connect to qipcrtr socket named {:?}", addr);
            SocketPeer::Address(address)
        }
    };
    let result = client.connect(current_task, peer.clone());

    if client.file().is_non_blocking() {
        return result;
    }

    match result {
        // EINPROGRESS may be returned for inet sockets when `connect()` is completed
        // asynchronously.
        Err(errno) if errno.code == EINPROGRESS => {
            let waiter = Waiter::new();
            client.file().wait_async(
                current_task,
                &waiter,
                FdEvents::POLLOUT,
                WaitCallback::none(),
            );
            if !client.file().query_events(current_task)?.contains(FdEvents::POLLOUT) {
                waiter.wait(current_task)?;
            }
            client.connect(current_task, peer)
        }
        // TODO(tbodt): Support blocking when the UNIX domain socket queue fills up. This one's
        // weird because as far as I can tell, removing a socket from the queue does not actually
        // trigger FdEvents on anything.
        result => result,
    }
}

fn write_socket_address(
    current_task: &CurrentTask,
    user_socket_address: UserAddress,
    user_address_length: UserRef<socklen_t>,
    address_bytes: &[u8],
) -> Result<(), Errno> {
    let capacity = current_task.read_object(user_address_length)?;
    if capacity > i32::MAX as socklen_t {
        return error!(EINVAL);
    }
    let length = address_bytes.len() as socklen_t;
    if length > 0 {
        let actual = std::cmp::min(length, capacity) as usize;
        current_task.write_memory(user_socket_address, &address_bytes[..actual])?;
    }
    current_task.write_object(user_address_length, &length)?;
    Ok(())
}

pub fn sys_getsockname(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_socket_address: UserAddress,
    user_address_length: UserRef<socklen_t>,
) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;
    security::check_socket_getsockname_access(current_task, socket)?;
    let address_bytes = socket.getsockname()?.to_bytes();

    write_socket_address(current_task, user_socket_address, user_address_length, &address_bytes)?;

    Ok(())
}

pub fn sys_getpeername(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_socket_address: UserAddress,
    user_address_length: UserRef<socklen_t>,
) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;
    security::check_socket_getpeername_access(current_task, socket)?;
    let address_bytes = socket.getpeername()?.to_bytes();

    write_socket_address(current_task, user_socket_address, user_address_length, &address_bytes)?;

    Ok(())
}

pub fn sys_socketpair(
    current_task: &CurrentTask,
    domain: u32,
    socket_type: u32,
    protocol: u32,
    user_sockets: UserRef<[FdNumber; 2]>,
) -> Result<(), Errno> {
    let flags = socket_type & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let domain = parse_socket_domain(domain)?;
    if !matches!(domain, SocketDomain::Unix | SocketDomain::Inet) {
        return error!(EAFNOSUPPORT);
    }
    let socket_type = parse_socket_type(domain, socket_type)?;
    let _protocol = parse_socket_protocol(domain, socket_type, protocol)?;
    if domain != SocketDomain::Unix {
        return error!(EOPNOTSUPP);
    }
    let open_flags = socket_flags_to_open_flags(flags);

    let (left, right) = UnixSocket::new_pair(current_task, domain, socket_type, open_flags)?;

    let fd_flags = socket_flags_to_fd_flags(flags);
    // TODO: Eventually this will need to allocate two fd numbers (each of which could
    // potentially fail), and only populate the fd numbers (which can't fail) if both allocations
    // succeed.
    let left_fd = current_task.add_file(left, fd_flags)?;
    let right_fd = current_task.add_file(right, fd_flags)?;

    let fds = [left_fd, right_fd];
    log_trace!("socketpair -> [{:#x}, {:#x}]", fds[0].raw(), fds[1].raw());
    current_task.write_object(user_sockets, &fds)?;

    Ok(())
}

fn read_iovec_from_msghdr(
    current_task: &CurrentTask,
    message_header: WithAlternateBuffer<&MsgHdr>,
) -> Result<UserBuffers, Errno> {
    if let WithAlternateBuffer::WithAux(_, b) = message_header {
        return Ok(UserBuffers::from_buf([b]));
    }
    let iovec_count = message_header.iovlen;

    // In `CurrentTask::read_iovec()` the same check fails with `EINVAL`. This works for all
    // syscalls that use `iovec`, except `sendmsg()` and `recvmsg()`, which need to fail with
    // EMSGSIZE.
    if iovec_count.raw() > UIO_MAXIOV as usize {
        return error!(EMSGSIZE);
    }

    current_task.read_iovec(message_header.iov, iovec_count)
}

fn recvmsg_internal(
    current_task: &CurrentTask,
    file: &FileHandle,
    user_message_header: &mut MsgHdrRef,
    flags: u32,
    deadline: Option<zx::MonotonicInstant>,
) -> Result<usize, Errno> {
    let mut message_header = match *user_message_header {
        MsgHdrRef::Ptr(ptr) => current_task.read_multi_arch_object(ptr)?.into(),
        MsgHdrRef::Value(ref value) => value.clone(),
    };
    let result =
        recvmsg_internal_with_header(current_task, file, message_header.as_mut(), flags, deadline)?;
    match *user_message_header {
        MsgHdrRef::Ptr(ptr) => {
            current_task.write_multi_arch_object(ptr, message_header.extract())?;
        }
        MsgHdrRef::Value(ref mut value) => {
            *value.deref_mut() = message_header.extract();
        }
    }
    Ok(result)
}

fn recvmsg_internal_with_header(
    current_task: &CurrentTask,
    file: &FileHandle,
    mut message_header: WithAlternateBuffer<&mut MsgHdr>,
    flags: u32,
    deadline: Option<zx::MonotonicInstant>,
) -> Result<usize, Errno> {
    let iovec = read_iovec_from_msghdr(current_task, message_header.as_unmut())?;

    let flags = SocketMessageFlags::from_bits(flags).ok_or_else(|| errno!(EINVAL))?;
    let socket_ops = file.downcast_file::<SocketFile>().unwrap();
    let info = socket_ops.recvmsg(
        current_task,
        file,
        &mut UserBuffersOutputBuffer::unified_new(current_task, iovec)?,
        flags,
        deadline,
    )?;

    message_header.flags = 0;

    let cmsg_buffer_size = message_header.control_len;

    let mut cmsg_bytes_written = 0;
    let header_size = CMsgHdrPtr::size_of_object_for(current_task);

    for ancillary_data in info.ancillary_data {
        if ancillary_data.total_size(current_task) == 0 {
            // Skip zero-byte ancillary data on the receiving end. Not doing this trips this
            // assert:
            // https://cs.android.com/android/platform/superproject/+/master:system/libbase/cmsg.cpp;l=144;drc=15ec2c7a23cda814351a064a345a8270ed8c83ab
            continue;
        }

        // Calculate the offset where the current message will be written, after alignment.
        let aligned_offset = cmsg_align(current_task, cmsg_bytes_written)?;
        let space_available = cmsg_buffer_size.saturating_sub(aligned_offset);

        if space_available < header_size {
            // Can't fit the header, so stop trying to write.
            message_header.flags |= MSG_CTRUNC;
            break;
        }

        let conversion = ancillary_data.into_bytes(current_task, flags, space_available)?;

        if conversion.truncated {
            message_header.flags |= MSG_CTRUNC;
        }

        assert!(aligned_offset + conversion.bytes.len() <= cmsg_buffer_size);
        // Write the message at the aligned offset.
        current_task.write_memory((message_header.control + aligned_offset)?, &conversion.bytes)?;
        // Update the total bytes written to the end of this message.
        cmsg_bytes_written = std::cmp::min(
            cmsg_align(current_task, aligned_offset + conversion.bytes.len())?,
            cmsg_buffer_size,
        );
    }

    message_header.control_len = cmsg_bytes_written;

    let msg_name = message_header.name;
    if !msg_name.is_null() {
        if message_header.name_len > i32::MAX as u32 {
            return error!(EINVAL);
        }
        let bytes = info.address.map(|a| a.to_bytes()).unwrap_or_else(|| vec![]);
        let num_bytes = std::cmp::min(message_header.name_len as usize, bytes.len());
        message_header.name_len = bytes.len() as u32;
        if num_bytes > 0 {
            current_task.write_memory(msg_name, &bytes[..num_bytes])?;
        }
    }

    if socket_ops.socket.socket_type != SocketType::Stream && info.bytes_read != info.message_length
    {
        message_header.flags |= MSG_TRUNC;
    }

    if flags.contains(SocketMessageFlags::TRUNC)
        && socket_ops.socket.socket_type != SocketType::Stream
    {
        Ok(info.message_length)
    } else {
        Ok(info.bytes_read)
    }
}

pub fn sys_recvmsg(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_message_header: MsgHdrPtr,
    flags: u32,
) -> Result<usize, Errno> {
    recvmsg_impl(current_task, fd, &mut user_message_header.into(), flags)
}

/// Implementation of `recvmsg`.
///
/// This function is used by `sys_recvmsg`, but can also be called from other parts of the kernel
/// that need to override the `iovec` from the `msghdr`. For example, when using `io_uring` with
/// ring buffers.
pub fn recvmsg_impl(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_message_header: &mut MsgHdrRef,
    flags: u32,
) -> Result<usize, Errno> {
    let file = current_task.files().get(fd)?;
    if !file.node().is_sock() {
        return error!(ENOTSOCK);
    }
    recvmsg_internal(current_task, &file, user_message_header, flags, None)
}

pub fn sys_recvmmsg(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_mmsgvec: MMsgHdrPtr,
    vlen: u32,
    mut flags: u32,
    user_timeout: TimeSpecPtr,
) -> Result<usize, Errno> {
    let file = current_task.files().get(fd)?;
    if !file.node().is_sock() {
        return error!(ENOTSOCK);
    }

    if vlen > UIO_MAXIOV {
        return error!(EINVAL);
    }

    let deadline = if user_timeout.is_null() {
        None
    } else {
        let ts = current_task.read_multi_arch_object(user_timeout)?;
        Some(zx::MonotonicInstant::after(duration_from_timespec(ts)?))
    };

    let mut index = 0usize;
    while index < vlen as usize {
        let current_ptr = user_mmsgvec.at(index)?;
        let mut current_mmsghdr = current_task.read_multi_arch_object(current_ptr)?;
        match recvmsg_internal_with_header(
            current_task,
            &file,
            (&mut current_mmsghdr.hdr).into(),
            flags,
            deadline,
        ) {
            Err(error) => {
                if index == 0 {
                    return Err(error);
                }
                break;
            }
            Ok(bytes_read) => {
                current_mmsghdr.len = bytes_read;
                current_task.write_multi_arch_object(current_ptr, current_mmsghdr)?;
            }
        }
        index += 1;
        if flags & MSG_WAITFORONE != 0 {
            flags |= MSG_DONTWAIT;
        }
    }
    Ok(index)
}

pub fn sys_recvfrom(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_buffer: UserAddress,
    buffer_length: usize,
    flags: u32,
    user_src_address: UserAddress,
    user_src_address_length: UserRef<socklen_t>,
) -> Result<usize, Errno> {
    let file = current_task.files().get(fd)?;
    if !file.node().is_sock() {
        return error!(ENOTSOCK);
    }

    let flags = SocketMessageFlags::from_bits(flags).ok_or_else(|| errno!(EINVAL))?;
    let socket_ops = file.downcast_file::<SocketFile>().unwrap();
    let info = socket_ops.recvmsg(
        current_task,
        &file,
        &mut UserBuffersOutputBuffer::unified_new_at(current_task, user_buffer, buffer_length)?,
        flags,
        None,
    )?;

    if !user_src_address.is_null() {
        let bytes = info.address.map(|a| a.to_bytes()).unwrap_or_else(|| vec![]);
        write_socket_address(current_task, user_src_address, user_src_address_length, &bytes)?;
    }

    if flags.contains(SocketMessageFlags::TRUNC)
        && socket_ops.socket.socket_type != SocketType::Stream
    {
        Ok(info.message_length)
    } else {
        Ok(info.bytes_read)
    }
}

fn sendmsg_internal(
    current_task: &CurrentTask,
    file: &FileHandle,
    user_message_header: MsgHdrPtr,
    flags: u32,
) -> Result<usize, Errno> {
    let message_header = current_task.read_multi_arch_object(user_message_header)?;
    sendmsg_internal_with_header(current_task, file, &message_header, flags)
}

fn sendmsg_internal_with_header(
    current_task: &CurrentTask,
    file: &FileHandle,
    message_header: &MsgHdr,
    flags: u32,
) -> Result<usize, Errno> {
    if message_header.name_len > i32::MAX as u32 {
        return error!(EINVAL);
    }
    if message_header.control_len > 20480 {
        return error!(ENOBUFS);
    }
    let dest_address = maybe_parse_socket_address(
        current_task,
        message_header.name,
        message_header.name_len as usize,
    )?;
    let iovec = read_iovec_from_msghdr(current_task, message_header.into())?;

    let mut next_message_offset: usize = 0;
    let mut ancillary_data = Vec::new();
    let header_size = CMsgHdrPtr::size_of_object_for(current_task);
    loop {
        let space = message_header.control_len.saturating_sub(next_message_offset);
        if space < header_size {
            break;
        }
        let cmsg_ref =
            CMsgHdrPtr::new(current_task, (message_header.control + next_message_offset)?);
        let cmsg = current_task.read_multi_arch_object(cmsg_ref)?;
        // If the message header is not long enough to fit the required fields of the
        // control data, return EINVAL.
        if (cmsg.cmsg_len as usize) < header_size {
            return error!(EINVAL);
        }

        let data_size = std::cmp::min(cmsg.cmsg_len as usize - header_size, space);
        let next_data_offset = next_message_offset + header_size;
        let data = current_task
            .read_memory_to_vec((message_header.control + next_data_offset)?, data_size)?;
        next_message_offset += cmsg_align(current_task, header_size + data.len())?;
        let data = AncillaryData::from_cmsg(
            current_task,
            ControlMsg::new(cmsg.cmsg_level, cmsg.cmsg_type, data),
        )?;
        if data.total_size(current_task) == 0 {
            continue;
        }
        ancillary_data.push(data);
    }

    let flags = SocketMessageFlags::from_bits(flags).ok_or_else(|| errno!(EOPNOTSUPP))?;
    let socket_ops = file.downcast_file::<SocketFile>().unwrap();
    socket_ops.sendmsg(
        current_task,
        file,
        &mut UserBuffersInputBuffer::unified_new(current_task, iovec)?,
        dest_address,
        ancillary_data,
        flags,
    )
}

pub fn sys_sendmsg(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_message_header: MsgHdrPtr,
    flags: u32,
) -> Result<usize, Errno> {
    let file = current_task.files().get(fd)?;
    if !file.node().is_sock() {
        return error!(ENOTSOCK);
    }
    sendmsg_internal(current_task, &file, user_message_header, flags)
}

pub fn sys_sendmmsg(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_mmsgvec: MMsgHdrPtr,
    mut vlen: u32,
    flags: u32,
) -> Result<usize, Errno> {
    let file = current_task.files().get(fd)?;
    if !file.node().is_sock() {
        return error!(ENOTSOCK);
    }

    // vlen is capped at UIO_MAXIOV.
    if vlen > UIO_MAXIOV {
        vlen = UIO_MAXIOV;
    }

    let mut index = 0usize;
    while index < vlen as usize {
        let current_ptr = user_mmsgvec.at(index)?;
        let mut current_mmsghdr = current_task.read_multi_arch_object(current_ptr)?;
        match sendmsg_internal_with_header(current_task, &file, &current_mmsghdr.hdr, flags) {
            Err(error) => {
                if index == 0 {
                    return Err(error);
                }
                break;
            }
            Ok(bytes_read) => {
                current_mmsghdr.len = bytes_read;
                current_task.write_multi_arch_object(current_ptr, current_mmsghdr)?;
            }
        }
        index += 1;
    }
    Ok(index)
}

pub fn sys_sendto(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_buffer: UserAddress,
    user_buffer_length: usize,
    flags: u32,
    user_dest_address: UserAddress,
    user_dest_address_length: socklen_t,
) -> Result<usize, Errno> {
    let file = current_task.files().get(fd)?;
    if !file.node().is_sock() {
        return error!(ENOTSOCK);
    }

    let dest_address = maybe_parse_socket_address(
        current_task,
        user_dest_address,
        user_dest_address_length as usize,
    )?;
    let mut data =
        UserBuffersInputBuffer::unified_new_at(current_task, user_buffer, user_buffer_length)?;

    let flags = SocketMessageFlags::from_bits(flags).ok_or_else(|| errno!(EOPNOTSUPP))?;
    let socket_file = file.downcast_file::<SocketFile>().unwrap();
    socket_file.sendmsg(current_task, &file, &mut data, dest_address, vec![], flags)
}

pub fn sys_getsockopt(
    current_task: &CurrentTask,
    fd: FdNumber,
    level: u32,
    optname: u32,
    user_optval: UserAddress,
    user_optlen: UserRef<socklen_t>,
) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;

    let optlen = current_task.read_object(user_optlen)? as usize;
    let optval_buffer_len = optlen;
    let mut optval = current_task.read_memory_to_vec(user_optval, optlen as usize)?;

    let result = if socket.domain.is_inet() && IpTables::can_handle_getsockopt(level, optname) {
        current_task.kernel().iptables().getsockopt(current_task, socket, optname, optval.clone())
    } else {
        socket.getsockopt(current_task, level, optname, optlen as u32)
    };

    // Even if `getsockopt()` above returned an error we still need to run
    // the eBPF program - it may handle the error.
    let (optlen, error) = match result {
        Ok(new_optval) => {
            // Linux getsockopt allows the user to pass a buffer smaller than the option's
            // actual size, in which case the kernel truncates the returned value to fit
            // the buffer and returns success.
            //
            // On the other hand, if the option is smaller than the user provided buffer,
            // the eBPF program can still use the entire user allocated buffer if needed.
            let len = std::cmp::min(new_optval.len(), optval.len());
            optval[..len].copy_from_slice(&new_optval[..len]);
            (len, None)
        }
        Err(e) => (optlen, Some(e)),
    };

    let root_cgroup = current_task.kernel().ebpf_state.attachments.root_cgroup();
    let (optval, optlen) = root_cgroup.run_getsockopt_prog(
        current_task,
        level,
        optname,
        optval,
        optlen,
        error,
        socket,
    )?;

    assert!(optlen <= optval_buffer_len);
    current_task.write_memory(user_optval, &optval[..optlen])?;
    current_task.write_object(user_optlen, &(optlen as u32))?;

    Ok(())
}

pub fn sys_setsockopt(
    current_task: &CurrentTask,
    fd: FdNumber,
    level: u32,
    optname: u32,
    user_optval: UserAddress,
    optlen: socklen_t,
) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;

    let user_opt = UserBuffer { address: user_optval, length: optlen as usize };

    // Run eBPF program if any.
    let root_cgroup = current_task.kernel().ebpf_state.attachments.root_cgroup();
    let optval = match root_cgroup.run_setsockopt_prog(
        current_task,
        level,
        optname,
        user_opt.into(),
        socket,
    ) {
        SetSockOptProgramResult::Allow(value) => value,
        SetSockOptProgramResult::Fail(errno) => return Err(errno),
        SetSockOptProgramResult::Bypass => return Ok(()), // The option was handled by eBPF.
    };

    if socket.domain.is_inet() && IpTables::can_handle_setsockopt(level, optname) {
        current_task.kernel().iptables().setsockopt(current_task, socket, optname, optval)
    } else {
        socket.setsockopt(current_task, level, optname, optval)
    }
}

pub fn sys_shutdown(current_task: &CurrentTask, fd: FdNumber, how: u32) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let socket = Socket::get_from_file(&file)?;
    let how = match how {
        SHUT_RD => SocketShutdownFlags::READ,
        SHUT_WR => SocketShutdownFlags::WRITE,
        SHUT_RDWR => SocketShutdownFlags::READ | SocketShutdownFlags::WRITE,
        _ => return error!(EINVAL),
    };
    socket.shutdown(current_task, how)?;
    Ok(())
}

pub fn cmsg_align(current_task: &CurrentTask, value: usize) -> Result<usize, Errno> {
    let alignment = if current_task.is_arch32() { 4 } else { 8 };
    round_up_to_increment(value, alignment)
}

// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    use crate::task::CurrentTask;
    use crate::vfs::FdNumber;
    use starnix_uapi::errors::Errno;
    use starnix_uapi::user_address::UserAddress;

    pub use super::{
        sys_accept as sys_arch32_accept, sys_accept4 as sys_arch32_accept4,
        sys_bind as sys_arch32_bind, sys_getpeername as sys_arch32_getpeername,
        sys_getsockname as sys_arch32_getsockname, sys_getsockopt as sys_arch32_getsockopt,
        sys_listen as sys_arch32_listen, sys_recvfrom as sys_arch32_recvfrom,
        sys_recvmmsg as sys_arch32_recvmmsg, sys_recvmsg as sys_arch32_recvmsg,
        sys_sendmsg as sys_arch32_sendmsg, sys_sendto as sys_arch32_sendto,
        sys_setsockopt as sys_arch32_setsockopt, sys_shutdown as sys_arch32_shutdown,
        sys_socketpair as sys_arch32_socketpair,
    };

    pub fn sys_arch32_send(
        current_task: &CurrentTask,
        fd: FdNumber,
        user_buffer: UserAddress,
        user_buffer_length: usize,
        flags: u32,
    ) -> Result<usize, Errno> {
        super::sys_sendto(
            current_task,
            fd,
            user_buffer,
            user_buffer_length,
            flags,
            Default::default(),
            Default::default(),
        )
    }

    pub fn sys_arch32_recv(
        current_task: &CurrentTask,
        fd: FdNumber,
        user_buffer: UserAddress,
        buffer_length: usize,
        flags: u32,
    ) -> Result<usize, Errno> {
        super::sys_recvfrom(
            current_task,
            fd,
            user_buffer,
            buffer_length,
            flags,
            Default::default(),
            Default::default(),
        )
    }
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_and_run;
    use starnix_uapi::{AF_INET, AF_UNIX, SOCK_STREAM};

    #[::fuchsia::test]
    async fn test_socketpair_invalid_arguments() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(
                sys_socketpair(
                    current_task,
                    AF_INET as u32,
                    SOCK_STREAM,
                    0,
                    UserRef::new(UserAddress::default())
                ),
                error!(EPROTONOSUPPORT)
            );
            assert_eq!(
                sys_socketpair(
                    current_task,
                    AF_UNIX as u32,
                    7,
                    0,
                    UserRef::new(UserAddress::default())
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_socketpair(
                    current_task,
                    AF_UNIX as u32,
                    SOCK_STREAM,
                    0,
                    UserRef::new(UserAddress::default())
                ),
                error!(EFAULT)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    fn test_generate_autobind_address() {
        let address = generate_autobind_address();
        assert_eq!(address.len(), 6);
        assert_eq!(address[0], 0);
        for byte in address[1..].iter() {
            match byte {
                b'0'..=b'9' | b'a'..=b'f' => {
                    // Ok.
                }
                bad => {
                    panic!("bad byte: {bad}");
                }
            }
        }
    }
}
