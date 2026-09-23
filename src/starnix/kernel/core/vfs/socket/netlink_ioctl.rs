// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{NetlinkFamily, SocketDomain, SocketFile, SocketProtocol, SocketType};
use crate::mm::MemoryAccessorExt;
use crate::security;
use crate::task::CurrentTask;
use crate::vfs::FileHandle;
use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
use byteorder::{ByteOrder as _, NativeEndian};
use starnix_uapi::{AF_INET, arch_struct_with_union};

use net_types::ip::IpAddress;
use netlink_packet_core::{ErrorMessage, NetlinkHeader, NetlinkMessage, NetlinkPayload};
use netlink_packet_route::address::{AddressAttribute, AddressMessage};
use netlink_packet_route::link::{LinkAttribute, LinkFlags, LinkMessage};
use netlink_packet_route::{AddressFamily, RouteNetlinkMessage, RouteNetlinkMessageParseMode};
use starnix_logging::{log_warn, track_stub};

use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::auth::CAP_NET_ADMIN;
use starnix_uapi::errors::{Errno, ErrnoCode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::union::struct_with_union_into_bytes;
use starnix_uapi::user_address::{ArchSpecific, UserAddress};
use starnix_uapi::{
    IFNAMSIZ, SIOCGIFADDR, SIOCGIFFLAGS, SIOCGIFHWADDR, SIOCGIFINDEX, SIOCGIFMTU, SIOCGIFNAME,
    SIOCGIFNETMASK, SIOCSIFADDR, SIOCSIFFLAGS, SIOCSIFNETMASK, arch_union_wrapper, c_char, errno,
    error, uapi,
};
use static_assertions::const_assert;
use std::ffi::CStr;
use std::net::IpAddr;
use zerocopy::{FromBytes as _, IntoBytes};

/// The size of a buffer suitable to carry netlink route messages.
const NETLINK_ROUTE_BUF_SIZE: usize = 1024;

arch_union_wrapper! {
    pub IfReq(ifreq);
}

impl IfReq {
    fn new_with_sockaddr<Arch: ArchSpecific>(
        arch: &Arch,
        name: &[uapi::c_char; 16],
        sockaddr: uapi::sockaddr,
    ) -> Self {
        Self(arch_struct_with_union!(arch, ifreq {
            ifr_ifrn.ifrn_name: name.clone(),
            ifr_ifru.ifru_addr: zerocopy::transmute!(sockaddr),
        }))
    }

    fn new_with_i32<Arch: ArchSpecific>(
        arch: &Arch,
        name: &[uapi::c_char; 16],
        value: i32,
    ) -> Self {
        Self(arch_struct_with_union!(arch, ifreq {
            ifr_ifrn.ifrn_name: name.clone(),
            ifr_ifru.ifru_ivalue: value,
        }))
    }

    fn new_with_flags<Arch: ArchSpecific>(
        arch: &Arch,
        name: &[uapi::c_char; 16],
        flags: i16,
    ) -> Self {
        Self(arch_struct_with_union!(arch, ifreq {
            ifr_ifrn.ifrn_name: name.clone(),
            ifr_ifru.ifru_flags: flags,
        }))
    }

    fn name(&self) -> &[uapi::c_char; 16] {
        // SAFETY Union is read with zerocopy, so all bytes are set.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        match self.inner() {
            IfReqInner::Arch64(ifreq) => unsafe { &ifreq.ifr_ifrn.ifrn_name },
            IfReqInner::Arch32(ifreq) => unsafe { &ifreq.ifr_ifrn.ifrn_name },
        }
    }

    pub fn name_as_str(&self) -> Result<&str, Errno> {
        let bytes: &[u8; 16] = zerocopy::transmute_ref!(self.name());
        let zero = bytes.iter().position(|x| *x == 0).ok_or_else(|| errno!(EINVAL))?;
        // SAFETY: This is safe as the zero was checked on the previous line.
        unsafe { CStr::from_bytes_with_nul_unchecked(&bytes[..zero + 1]) }
            .to_str()
            .map_err(|_| errno!(EINVAL))
    }

    fn ifru_addr(&self) -> &uapi::sockaddr {
        // SAFETY Union is read with zerocopy, so all bytes are set.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        match self.inner() {
            IfReqInner::Arch64(ifreq) => unsafe { &ifreq.ifr_ifru.ifru_addr },
            IfReqInner::Arch32(ifreq) => unsafe {
                zerocopy::transmute_ref!(&ifreq.ifr_ifru.ifru_addr)
            },
        }
    }

    fn ifru_netmask(&self) -> &uapi::sockaddr {
        // All sockaddr are equivalent
        self.ifru_addr()
    }

    pub fn ifru_flags(&self) -> i16 {
        // SAFETY Union is read with zerocopy, so all bytes are set.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        match self.inner() {
            IfReqInner::Arch64(ifreq) => unsafe { ifreq.ifr_ifru.ifru_flags },
            IfReqInner::Arch32(ifreq) => unsafe { ifreq.ifr_ifru.ifru_flags },
        }
    }

    pub fn ifru_ivalue(&self) -> i32 {
        // SAFETY Union is read with zerocopy, so all bytes are set.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        match self.inner() {
            IfReqInner::Arch64(ifreq) => unsafe { ifreq.ifr_ifru.ifru_ivalue },
            IfReqInner::Arch32(ifreq) => unsafe { ifreq.ifr_ifru.ifru_ivalue },
        }
    }
}

fn name_as_bytes(name: &String) -> Result<[uapi::c_char; 16], Errno> {
    // Perform an `as` cast rather than `try_into` because:
    //   - IFNAMSIZ is a known constant
    if name.len() >= IFNAMSIZ as usize {
        return Err(errno!(ENAMETOOLONG));
    }
    let name_bytes = name.as_bytes();
    // No explicit null terminator is needed because of the name length
    // being less than IFNAMSIZ. The last byte in the array will maintain
    // its zero value.
    let mut bytes = [0u8; 16];
    let dest_slice = &mut bytes[0..name_bytes.len()];
    dest_slice.copy_from_slice(name_bytes);
    let bytes: [uapi::c_char; 16] = zerocopy::transmute!(bytes);
    Ok(bytes)
}

pub fn netlink_ioctl(
    current_task: &CurrentTask,
    request: u32,
    arg: SyscallArg,
) -> Result<SyscallResult, Errno> {
    // Capability checks must be validated against the user task's credentials
    // before performing internal operations.
    match request {
        SIOCSIFADDR | SIOCSIFNETMASK | SIOCSIFFLAGS => {
            security::check_task_capable(current_task, CAP_NET_ADMIN)?;
        }
        _ => {}
    }

    let user_addr = UserAddress::from(arg);

    // TODO(https://fxbug.dev/42079507): Share this implementation with `fdio`
    // by moving things to `zxio`.

    // The following netdevice IOCTLs are supported on all sockets for
    // compatibility with Linux.
    //
    // Per https://man7.org/linux/man-pages/man7/netdevice.7.html,
    //
    //     Linux supports some standard ioctls to configure network devices.
    //     They can be used on any socket's file descriptor regardless of
    //     the family or type.
    let socket = new_route_socket(current_task)?;

    match request {
        SIOCGIFADDR => {
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let (address_msgs, _if_index) =
                get_netlink_ipv4_addresses(current_task, &socket, &in_ifreq, &mut read_buf)?;
            let mut maybe_errno = None;
            let ifru_addr = {
                let mut addr = uapi::sockaddr::default();
                let s_addr = address_msgs
                    .into_iter()
                    .next()
                    .and_then(|msg| {
                        msg.attributes.into_iter().find_map(|nla| {
                            if let AddressAttribute::Address(bytes) = nla {
                                // The bytes are held in network-endian
                                // order and `in_addr_t` is documented to
                                // hold values in network order as well. Per
                                // POSIX specifications for `sockaddr_in`
                                // https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/netinet_in.h.html.
                                //
                                //   The sin_port and sin_addr members shall
                                //   be in network byte order.
                                //
                                // Because of this, we read the bytes in
                                // native endian which is effectively a
                                // `core::mem::transmute` to `u32`.
                                Some(NativeEndian::read_u32(&match bytes {
                                    std::net::IpAddr::V4(v4) => v4.octets(),
                                    std::net::IpAddr::V6(_) => {
                                        maybe_errno =
                                            Some(error!(EINVAL, "expected an ipv4 address"));
                                        return None;
                                    }
                                }))
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);
                if let Some(errno) = maybe_errno {
                    return errno;
                }
                let _ = uapi::sockaddr_in {
                    sin_family: AF_INET,
                    sin_port: 0,
                    sin_addr: uapi::in_addr { s_addr },
                    __pad: Default::default(),
                }
                .write_to_prefix(addr.as_mut_bytes());
                addr
            };

            let out_ifreq = IfReq::new_with_sockaddr(current_task, in_ifreq.name(), ifru_addr);
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCGIFNETMASK => {
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let (address_msgs, _if_index) =
                get_netlink_ipv4_addresses(current_task, &socket, &in_ifreq, &mut read_buf)?;

            let mut maybe_errno = None;
            let ifru_netmask = {
                let mut addr = uapi::sockaddr::default();
                let s_addr = address_msgs
                    .into_iter()
                    .next()
                    .and_then(|msg| {
                        let prefix_len = msg.header.prefix_len;
                        if prefix_len > 32 {
                            maybe_errno = Some(error!(EINVAL, "invalid prefix length"));
                            return None;
                        }
                        // Convert prefix length to netmask.
                        let all_ones_address = net_types::ip::Ipv4Addr::new([u8::MAX; 4]);
                        let netmask = all_ones_address.mask(prefix_len);

                        // The bytes of the netmask are already in network byte order.
                        Some(NativeEndian::read_u32(netmask.bytes()))
                    })
                    .unwrap_or(0);

                if let Some(errno) = maybe_errno {
                    return errno;
                }

                let _ = uapi::sockaddr_in {
                    sin_family: AF_INET,
                    sin_port: 0,
                    sin_addr: uapi::in_addr { s_addr },
                    __pad: Default::default(),
                }
                .write_to_prefix(addr.as_mut_bytes());
                addr
            };

            let out_ifreq = IfReq::new_with_sockaddr(current_task, in_ifreq.name(), ifru_netmask);
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCGIFNAME => {
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            // SIOCGIFNAME calls provide the interface index and expect to
            // retrieve the interface name.
            let link_msg = get_netlink_interface_info_with_index(
                current_task,
                &socket,
                &in_ifreq,
                &mut read_buf,
            )?;

            // Find the interface name attribute. We expect only one.
            let if_name = link_msg.attributes.iter().find_map(|attr| {
                if let LinkAttribute::IfName(name) = attr { Some(name) } else { None }
            });

            let Some(name) = if_name else {
                return error!(ENODEV, "no device available for provided id");
            };

            // We pass back the same interface index that was originally provided.
            let out_ifreq =
                IfReq::new_with_i32(current_task, &name_as_bytes(name)?, in_ifreq.ifru_ivalue());
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCSIFADDR => {
            security::check_task_capable(current_task, CAP_NET_ADMIN)?;

            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let (address_msgs, if_index) =
                get_netlink_ipv4_addresses(current_task, &socket, &in_ifreq, &mut read_buf)?;

            let request_header = {
                let mut header = NetlinkHeader::default();
                // Always request the ACK response so that we know the
                // request has been handled before we return from this
                // operation.
                header.flags = netlink_packet_core::NLM_F_REQUEST | netlink_packet_core::NLM_F_ACK;
                header
            };

            // Helper to verify the response of a Netlink request
            let expect_ack = |msg: NetlinkMessage<RouteNetlinkMessage>| {
                match msg.payload {
                    NetlinkPayload::Error(ErrorMessage { code: Some(code), header: _, .. }) => {
                        // Don't propagate the error up because its not the fault of the
                        // caller - the stack state can change underneath the caller.
                        log_warn!(
                            "got NACK netlink route response when handling ioctl(_, {:#x}, _): {}",
                            request,
                            code
                        );
                    }
                    // `ErrorMessage` with no code represents an ACK.
                    NetlinkPayload::Error(ErrorMessage { code: None, header: _, .. }) => {}
                    payload => panic!("unexpected message = {:?}", payload),
                }
            };

            // Remove the first IPv4 address for the requested interface, if there is one.
            for addr in address_msgs.into_iter().take(1) {
                let resp = send_netlink_msg_and_wait_response(
                    current_task,
                    &socket,
                    NetlinkMessage::new(
                        request_header,
                        NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelAddress(addr)),
                    ),
                    &mut read_buf,
                )?;
                expect_ack(resp);
            }

            // Next, add the requested address.
            const_assert!(size_of::<uapi::sockaddr_in>() <= size_of::<uapi::sockaddr>());
            let addr = uapi::sockaddr_in::read_from_prefix(in_ifreq.ifru_addr().as_bytes())
                .expect("sockaddr_in is smaller than sockaddr")
                .0
                .sin_addr
                .s_addr;
            if addr != 0 {
                let resp = send_netlink_msg_and_wait_response(
                    current_task,
                    &socket,
                    NetlinkMessage::new(
                        request_header,
                        NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewAddress({
                            let mut msg = AddressMessage::default();
                            msg.header.family = AddressFamily::Inet.into();
                            msg.header.index = if_index.into();

                            // The SIOCSIFADDR ioctl already provides the address to set in
                            // network byte order.
                            let addr = addr.to_ne_bytes();
                            // The request does not include the prefix
                            // length so we use the default prefix for the
                            // address's class.
                            msg.header.prefix_len = net_types::ip::Ipv4Addr::new(addr)
                                .class()
                                .default_prefix_len()
                                .unwrap_or(net_types::ip::Ipv4Addr::BYTES * 8);
                            msg.attributes =
                                vec![AddressAttribute::Address(IpAddr::V4(addr.into()))];
                            msg
                        })),
                    ),
                    &mut read_buf,
                )?;
                expect_ack(resp);
            }

            Ok(SUCCESS)
        }
        SIOCSIFNETMASK => {
            security::check_task_capable(current_task, CAP_NET_ADMIN)?;

            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            const_assert!(size_of::<uapi::sockaddr_in>() <= size_of::<uapi::sockaddr>());
            let addr = uapi::sockaddr_in::read_from_prefix(in_ifreq.ifru_netmask().as_bytes())
                .expect("sockaddr_in is smaller than sockaddr")
                .0
                .sin_addr
                .s_addr;
            let prefix_len = addr.count_ones() as u8;
            // Check that the subnet is valid. The netmask is already in network byte order.
            match net_types::ip::Subnet::new(
                net_types::ip::Ipv4Addr::new(addr.to_ne_bytes()),
                prefix_len,
            ) {
                Ok(_) => (),
                Err(_) => {
                    return error!(EINVAL, "invalid netmask: {addr:?}");
                }
            }

            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let (address_msgs, _if_index) =
                get_netlink_ipv4_addresses(current_task, &socket, &in_ifreq, &mut read_buf)?;

            let request_header = {
                let mut header = NetlinkHeader::default();
                // Always request the ACK response so that we know the
                // request has been handled before we return from this
                // operation.
                header.flags = netlink_packet_core::NLM_F_REQUEST | netlink_packet_core::NLM_F_ACK;
                header
            };

            // Helper to verify the response of a Netlink request
            let expect_ack = |msg: NetlinkMessage<RouteNetlinkMessage>| {
                match msg.payload {
                    NetlinkPayload::Error(ErrorMessage { code: Some(code), header: _, .. }) => {
                        // Don't propagate the error up because its not the fault of the
                        // caller - the stack state can change underneath the caller.
                        log_warn!(
                            "got NACK netlink route response when handling ioctl(_, {:#x}, _): {}",
                            request,
                            code
                        );
                    }
                    // `ErrorMessage` with no code represents an ACK.
                    NetlinkPayload::Error(ErrorMessage { code: None, header: _, .. }) => {}
                    payload => panic!("unexpected message = {:?}", payload),
                }
            };

            // Remove the first IPv4 address on the requested interface.
            let Some(addr) = address_msgs.into_iter().next() else {
                // There's nothing to do if there are no addresses on the interface.
                return error!(EADDRNOTAVAIL, "no addresses to remove");
            };

            let resp = send_netlink_msg_and_wait_response(
                current_task,
                &socket,
                NetlinkMessage::new(
                    request_header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelAddress(addr.clone())),
                ),
                &mut read_buf,
            )?;
            expect_ack(resp);

            // Then, re-add it with the new netmask.
            let resp = send_netlink_msg_and_wait_response(
                current_task,
                &socket,
                NetlinkMessage::new(
                    request_header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewAddress({
                        let mut msg = addr;
                        msg.header.prefix_len = prefix_len;
                        msg
                    })),
                ),
                &mut read_buf,
            )?;
            expect_ack(resp);
            Ok(SUCCESS)
        }
        SIOCGIFHWADDR => {
            let user_addr = UserAddress::from(arg);
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let link_msg = get_netlink_interface_info_with_name(
                current_task,
                &socket,
                &in_ifreq,
                &mut read_buf,
            )?;

            let hw_addr_and_type = {
                let hw_type = link_msg.header.link_layer_type;
                link_msg.attributes.into_iter().find_map(|nla| {
                    if let LinkAttribute::Address(addr) = nla {
                        Some((addr, hw_type))
                    } else {
                        None
                    }
                })
            };

            let ifru_hwaddr = hw_addr_and_type
                .map(|(addr_bytes, sa_family)| {
                    let mut addr =
                        uapi::sockaddr { sa_family: sa_family.into(), sa_data: Default::default() };
                    // We need to manually assign from one to the other
                    // because we may be copying a vector of `u8` into
                    // an array of `i8` and regular `copy_from_slice`
                    // expects both src/dst slices to have the same
                    // element type.
                    //
                    // See /src/starnix/lib/linux_uapi/src/types.rs,
                    // `c_char` is an `i8` on `x86_64` and a `u8` on
                    // `arm64` and `riscv`.
                    addr.sa_data.iter_mut().zip(addr_bytes).for_each(
                        |(sa_data_byte, link_addr_byte): (&mut c_char, u8)| {
                            *sa_data_byte = link_addr_byte as c_char;
                        },
                    );
                    addr
                })
                .unwrap_or_else(Default::default);

            let out_ifreq = IfReq::new_with_sockaddr(current_task, in_ifreq.name(), ifru_hwaddr);
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCGIFINDEX => {
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let link_msg = get_netlink_interface_info_with_name(
                current_task,
                &socket,
                &in_ifreq,
                &mut read_buf,
            )?;
            let index =
                i32::try_from(link_msg.header.index).expect("interface ID should fit in an i32");
            let out_ifreq = IfReq::new_with_i32(current_task, in_ifreq.name(), index);
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCGIFMTU => {
            track_stub!(TODO("https://fxbug.dev/297369462"), "return actual socket MTU");
            let ifru_mtu = 1280; /* IPv6 MIN MTU */
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let out_ifreq = IfReq::new_with_i32(current_task, in_ifreq.name(), ifru_mtu);
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCGIFFLAGS => {
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
            let link_msg = get_netlink_interface_info_with_name(
                current_task,
                &socket,
                &in_ifreq,
                &mut read_buf,
            )?;
            // Perform an `as` cast rather than `try_into` because:
            //   - flags are a bit mask and should not be
            //     interpreted as negative,
            //   - SIOCGIFFLAGS returns a subset of the flags
            //     returned by netlink; the flags lost by truncating
            //     from 32 to 16 bits is expected.
            let flags_as_i16 = link_msg.header.flags.bits() as i16;

            let out_ifreq = IfReq::new_with_flags(current_task, in_ifreq.name(), flags_as_i16);
            current_task
                .write_multi_arch_object(IfReqPtr::new(current_task, user_addr), out_ifreq)?;
            Ok(SUCCESS)
        }
        SIOCSIFFLAGS => {
            security::check_task_capable(current_task, CAP_NET_ADMIN)?;
            let user_addr = UserAddress::from(arg);
            let in_ifreq: IfReq =
                current_task.read_multi_arch_object(IfReqPtr::new(current_task, user_addr))?;
            set_netlink_interface_flags(current_task, &socket, &in_ifreq).map(|()| SUCCESS)
        }
        _ => error!(ENOTTY),
    }
}

/// Performs an `RTM_GETLINK` request on `socket` for the interface requested in `in_ifreq`
/// using the iface name.
///
/// Returns the interface's information, or an [`Errno`] if the operation failed.
fn get_netlink_interface_info_with_name(
    current_task: &CurrentTask,
    socket: &FileHandle,
    in_ifreq: &IfReq,
    read_buf: &mut VecOutputBuffer,
) -> Result<LinkMessage, Errno> {
    let iface_name = in_ifreq.name_as_str()?;
    // Send the request to get the link details with the requested
    // interface name.
    let msg = NetlinkMessage::new(
        {
            let mut header = NetlinkHeader::default();
            header.flags = netlink_packet_core::NLM_F_REQUEST;
            header
        },
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetLink({
            let mut msg = LinkMessage::default();
            msg.attributes = vec![LinkAttribute::IfName(iface_name.to_string())];
            msg
        })),
    );
    get_netlink_interface_info(current_task, socket, read_buf, msg)
}

/// Performs an `RTM_GETLINK` request on `socket` for the interface requested in `in_ifreq`
/// using the iface index.
///
/// Returns the interface's information, or an [`Errno`] if the operation failed.
fn get_netlink_interface_info_with_index(
    current_task: &CurrentTask,
    socket: &FileHandle,
    in_ifreq: &IfReq,
    read_buf: &mut VecOutputBuffer,
) -> Result<LinkMessage, Errno> {
    // Get the if_index which is stored in the "ivalue" field.
    let index: i32 = in_ifreq.ifru_ivalue();
    // Perform an `as` cast rather than `try_into` because:
    //   - index must reflect a positive integer
    let index: u32 = index as u32;

    // Send the request to get the link details with the requested
    // interface index.
    let msg = NetlinkMessage::new(
        {
            let mut header = NetlinkHeader::default();
            header.flags = netlink_packet_core::NLM_F_REQUEST;
            header
        },
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetLink({
            let mut msg = LinkMessage::default();
            msg.header.index = index;
            msg
        })),
    );
    get_netlink_interface_info(current_task, socket, read_buf, msg)
}

fn new_route_socket(current_task: &CurrentTask) -> Result<FileHandle, Errno> {
    let internal_creds = security::creds_start_internal_operation(current_task);
    current_task.override_creds(internal_creds, || {
        SocketFile::new_socket(
            current_task,
            SocketDomain::Netlink,
            SocketType::Datagram,
            OpenFlags::RDWR,
            SocketProtocol::from_raw(NetlinkFamily::Route.as_raw()),
            /* kernel_private=*/ true,
        )
    })
}

// Helper function for getting an interface's info through Netlink
// using the supplied NetlinkMessage.
fn get_netlink_interface_info(
    current_task: &CurrentTask,
    socket: &FileHandle,
    read_buf: &mut VecOutputBuffer,
    msg: NetlinkMessage<RouteNetlinkMessage>,
) -> Result<LinkMessage, Errno> {
    let resp = send_netlink_msg_and_wait_response(current_task, socket, msg, read_buf)?;
    let link_msg = match resp.payload {
        NetlinkPayload::Error(ErrorMessage { code: Some(code), header: _, .. }) => {
            // `code` is an `i32` and may hold negative values so
            // we need to do an `as u64` cast instead of `try_into`.
            // Note that `ErrnoCode::from_return_value` will
            // cast the value to an `i64` to check that it is a
            // valid (negative) errno value.
            let code = ErrnoCode::from_return_value(code.get() as u64);
            return Err(Errno::with_context(code, "error code from RTM_GETLINK"));
        }
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(msg)) => msg,
        // netlink is only expected to return an error or
        // RTM_NEWLINK response for our RTM_GETLINK request.
        payload => panic!("unexpected message = {:?}", payload),
    };
    Ok(link_msg)
}

/// Performs an `RTM_GETADDR` dump request on `socket` for the requested interface
/// requested in `in_ifreq`.
///
/// Returns the list of addresses and interface index, or an [`Errno`] if the operation failed.
fn get_netlink_ipv4_addresses(
    current_task: &CurrentTask,
    socket: &FileHandle,
    in_ifreq: &IfReq,
    read_buf: &mut VecOutputBuffer,
) -> Result<(Vec<AddressMessage>, u32), Errno> {
    let uapi::sockaddr { sa_family, sa_data: _ } = in_ifreq.ifru_addr();
    if *sa_family != AF_INET {
        return error!(EINVAL);
    }

    let link_msg = get_netlink_interface_info_with_name(current_task, socket, in_ifreq, read_buf)?;
    let if_index = link_msg.header.index;

    // Send the request to dump all IPv4 addresses.
    let mut msg = NetlinkMessage::new(
        {
            let mut header = NetlinkHeader::default();
            header.flags = netlink_packet_core::NLM_F_DUMP | netlink_packet_core::NLM_F_REQUEST;
            header
        },
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetAddress({
            let mut msg = AddressMessage::default();
            msg.header.family = AddressFamily::Inet.into();
            msg
        })),
    );
    msg.finalize();
    let mut buf = vec![0; msg.buffer_len()];
    msg.serialize(&mut buf[..]);

    let internal_creds = security::creds_start_internal_operation(current_task);
    let addrs = current_task.override_creds(internal_creds, || {
        assert_eq!(socket.write(current_task, &mut VecInputBuffer::from(buf))?, msg.buffer_len());

        // Collect all the addresses.
        let mut addrs = Vec::new();
        loop {
            read_buf.reset();
            let n = socket.read(current_task, read_buf)?;

            let msg = NetlinkMessage::<RouteNetlinkMessage>::deserialize(
                &read_buf.data()[..n],
                RouteNetlinkMessageParseMode::Strict,
            )
            .expect("netlink should always send well-formed messages");
            match msg.payload {
                NetlinkPayload::Done(_) => break,
                NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewAddress(msg)) => {
                    if msg.header.index == if_index {
                        addrs.push(msg);
                    }
                }
                payload => panic!("unexpected message = {:?}", payload),
            }
        }
        Ok(addrs)
    })?;

    Ok((addrs, if_index))
}

/// Performs `RTM_SETLINK` on `socket` to update the flags.
fn set_netlink_interface_flags(
    current_task: &CurrentTask,
    socket: &FileHandle,
    in_ifreq: &IfReq,
) -> Result<(), Errno> {
    let iface_name = in_ifreq.name_as_str()?;
    let flags: i16 = in_ifreq.ifru_flags();
    // Perform an `as` cast rather than `try_into` because:
    //   - flags are a bit mask and should not be interpreted as negative,
    //   - no loss in precision when upcasting 16 bits to 32 bits.
    let flags: u32 = flags as u32;

    // Send the request to set the link flags with the requested interface name.
    let msg = NetlinkMessage::new(
        {
            let mut header = NetlinkHeader::default();
            header.flags = netlink_packet_core::NLM_F_REQUEST | netlink_packet_core::NLM_F_ACK;
            header
        },
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::SetLink({
            let mut msg = LinkMessage::default();
            msg.header.flags = LinkFlags::from_bits(flags).unwrap();
            // Only attempt to change flags in the first 16 bits, because
            // `ifreq` represents flags as a short (i16).
            msg.header.change_mask = LinkFlags::from_bits(u16::MAX as u32).unwrap();
            msg.attributes = vec![LinkAttribute::IfName(iface_name.to_string())];
            msg
        })),
    );
    let mut read_buf = VecOutputBuffer::new(NETLINK_ROUTE_BUF_SIZE);
    let resp = send_netlink_msg_and_wait_response(current_task, socket, msg, &mut read_buf)?;
    match resp.payload {
        NetlinkPayload::Error(ErrorMessage { code: Some(code), header: _, .. }) => {
            // `code` is an `i32` and may hold negative values so
            // we need to do an `as u64` cast instead of `try_into`.
            // Note that `ErrnoCode::from_return_value` will
            // cast the value to an `i64` to check that it is a
            // valid (negative) errno value.
            let code = ErrnoCode::from_return_value(code.get() as u64);
            Err(Errno::with_context(code, "error code from RTM_SETLINK"))
        }
        // `ErrorMessage` with no code represents an ACK.
        NetlinkPayload::Error(ErrorMessage { code: None, header: _, .. }) => Ok(()),
        // Netlink is only expected to return an error or an ack.
        payload => panic!("unexpected message = {:?}", payload),
    }
}

/// Sends the msg on the provided NETLINK ROUTE socket, returning the response.
fn send_netlink_msg_and_wait_response(
    current_task: &CurrentTask,
    socket: &FileHandle,
    mut msg: NetlinkMessage<RouteNetlinkMessage>,
    read_buf: &mut VecOutputBuffer,
) -> Result<NetlinkMessage<RouteNetlinkMessage>, Errno> {
    msg.finalize();
    let mut buf = vec![0; msg.buffer_len()];
    msg.serialize(&mut buf[..]);

    let internal_creds = security::creds_start_internal_operation(current_task);
    current_task.override_creds(internal_creds, || {
        assert_eq!(socket.write(current_task, &mut VecInputBuffer::from(buf))?, msg.buffer_len());

        read_buf.reset();
        let n = socket.read(current_task, read_buf)?;
        let msg = NetlinkMessage::<RouteNetlinkMessage>::deserialize(
            &read_buf.data()[..n],
            RouteNetlinkMessageParseMode::Strict,
        )
        .expect("netlink should always send well-formed messages");
        Ok(msg)
    })
}
