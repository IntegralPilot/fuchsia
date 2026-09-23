// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::audit::Auditable;
use super::fs_node::compute_new_fs_node_sid;
use super::{
    build_permission_check, check_permission, current_task_state, fs_node_effective_sid_and_class,
};
use crate::security::selinux_hooks::{FsNodeSidAndClass, superblock};
use crate::task::CurrentTask;
use crate::vfs::socket::{
    NetlinkFamily, Socket, SocketAddress, SocketDomain, SocketFile, SocketPeer, SocketProtocol,
    SocketShutdownFlags, SocketType, socket_fs,
};
use crate::vfs::{DowncastedFile, FsNode};
use selinux::permission_check::PermissionCheck;
use selinux::{
    CommonFsNodePermission, CommonSocketPermission, ForClass, FsNodeClass, InitialSid,
    KernelPermission, PolicyCap, SecurityId, SecurityServer, SocketClass,
    UnixStreamSocketPermission,
};
use starnix_logging::track_stub;
use starnix_uapi::errors::Errno;

/// Checks that `current_task` has the specified `permission` for the `socket_node`.
pub(super) fn has_socket_permission(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    socket_node: &FsNode,
    permission: impl ForClass<SocketClass>,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    // Permissions are allowed for kernel sockets.
    if socket_node.is_private() {
        return Ok(());
    }

    let FsNodeSidAndClass { sid: socket_sid, class: socket_class } =
        fs_node_effective_sid_and_class(socket_node);
    let FsNodeClass::Socket(socket_class) = socket_class else {
        panic!("socket API called for non-Socket class")
    };

    let audit_context = [audit_context, socket_node.into()];
    check_permission(
        permission_check,
        current_task,
        subject_sid,
        socket_sid,
        permission.for_class(socket_class),
        (&audit_context).into(),
    )
}

/// Computes the socket security class for `domain`, `socket_type` and `protocol`.
fn compute_socket_security_class(
    security_server: &SecurityServer,
    domain: SocketDomain,
    socket_type: SocketType,
    protocol: SocketProtocol,
) -> SocketClass {
    let use_extended_classes =
        || security_server.is_policycap_enabled(PolicyCap::ExtendedSocketClass);
    match domain {
        SocketDomain::Unix => match socket_type {
            SocketType::Stream | SocketType::SeqPacket => SocketClass::UnixStreamSocket,
            SocketType::Raw | SocketType::Datagram => SocketClass::UnixDgramSocket,

            // This combination of domain & type has no unique security class.
            SocketType::Rdm | SocketType::Dccp | SocketType::Packet => SocketClass::Socket,
        },
        SocketDomain::Inet | SocketDomain::Inet6 => match socket_type {
            SocketType::Stream => match protocol {
                SocketProtocol::IP | SocketProtocol::TCP => SocketClass::TcpSocket,

                // Protocols other than TCP receive a dedicated security class if extended socket
                // classes are enabled in the policy.
                SocketProtocol::SCTP if use_extended_classes() => SocketClass::SctpSocket,

                // Otherwise allow protocols to be treated "rawip_socket", pending a dedicated
                // security class being introduced.
                _ => SocketClass::RawIpSocket,
            },
            SocketType::Datagram => match protocol {
                SocketProtocol::IP | SocketProtocol::UDP | SocketProtocol::UDPLITE => {
                    SocketClass::UdpSocket
                }

                // Protocols other than UDP & UDP-Lite receive a dedicated security class if
                // extended socket classes are enabled in the policy.
                SocketProtocol::ICMP | SocketProtocol::ICMPV6 if use_extended_classes() => {
                    SocketClass::IcmpSocket
                }

                // Otherwise allow protocols to be treated "rawip_socket", pending a dedicated
                // security class being introduced.
                _ => SocketClass::RawIpSocket,
            },
            SocketType::Raw => SocketClass::RawIpSocket,

            // This combination of domain & type has no unique security class, so default to the
            // "rawip_socket" class until/unless some more specific class is introduced.
            SocketType::SeqPacket | SocketType::Rdm | SocketType::Dccp | SocketType::Packet => {
                SocketClass::RawIpSocket
            }
        },
        SocketDomain::Netlink => match NetlinkFamily::from_raw(protocol.as_raw()) {
            NetlinkFamily::Route => SocketClass::NetlinkRouteSocket,
            NetlinkFamily::Firewall => SocketClass::NetlinkFirewallSocket,
            NetlinkFamily::SockDiag => SocketClass::NetlinkTcpDiagSocket,
            NetlinkFamily::Nflog => SocketClass::NetlinkNflogSocket,
            NetlinkFamily::Xfrm => SocketClass::NetlinkXfrmSocket,
            NetlinkFamily::Selinux => SocketClass::NetlinkSelinuxSocket,
            NetlinkFamily::Iscsi => SocketClass::NetlinkIscsiSocket,
            NetlinkFamily::Audit => SocketClass::NetlinkAuditSocket,
            NetlinkFamily::FibLookup => SocketClass::NetlinkFibLookupSocket,
            NetlinkFamily::Connector => SocketClass::NetlinkConnectorSocket,
            NetlinkFamily::Netfilter => SocketClass::NetlinkNetfilterSocket,
            NetlinkFamily::Ip6Fw => SocketClass::NetlinkIp6FwSocket,
            NetlinkFamily::Dnrtmsg => SocketClass::NetlinkDnrtSocket,
            NetlinkFamily::KobjectUevent => SocketClass::NetlinkKobjectUeventSocket,
            NetlinkFamily::Generic => SocketClass::NetlinkGenericSocket,
            NetlinkFamily::Scsitransport => SocketClass::NetlinkScsitransportSocket,
            NetlinkFamily::Rdma => SocketClass::NetlinkRdmaSocket,
            NetlinkFamily::Crypto => SocketClass::NetlinkCryptoSocket,

            // No specific netlink security class equivalent.
            NetlinkFamily::Ecryptfs
            | NetlinkFamily::Smc
            | NetlinkFamily::Usersock
            | NetlinkFamily::Invalid => SocketClass::NetlinkSocket,
        },
        SocketDomain::Vsock if use_extended_classes() => SocketClass::VsockSocket,
        SocketDomain::Qipcrtr if use_extended_classes() => SocketClass::QipcrtrSocket,
        SocketDomain::Vsock | SocketDomain::Qipcrtr => SocketClass::Socket,
        SocketDomain::Packet => SocketClass::PacketSocket,
        SocketDomain::Key => SocketClass::KeySocket,
    }
}

/// Checks that `current_task` has permission to create a socket with `domain`, `socket_type` and
/// `protocol`.

pub(in crate::security) fn check_socket_create_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    domain: SocketDomain,
    socket_type: SocketType,
    protocol: SocketProtocol,
    kernel_private: bool,
) -> Result<(), Errno> {
    // Creating kernel sockets is allowed.
    if kernel_private {
        return Ok(());
    }

    let sockfs = socket_fs(current_task.kernel());
    // Ensure sockfs gets labeled, in case it was mounted after the SELinux policy has been loaded.
    superblock::file_system_resolve_security(security_server, &current_task, &sockfs)
        .expect("resolve fs security");
    let current_sid = current_task_state(current_task).current_sid;
    let new_socket_class =
        compute_socket_security_class(security_server, domain, socket_type, protocol);
    let new_socket_sid = if let Some(fs_label) = sockfs.security_state.state.label() {
        compute_new_fs_node_sid(
            security_server,
            current_task,
            fs_label,
            None,
            new_socket_class.into(),
            "".into(),
        )?
    } else {
        // TODO: https://fxbug.dev/364569053 - default to socket-related initial SIDs.
        InitialSid::Unlabeled.into()
    };

    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        new_socket_sid,
        CommonFsNodePermission::Create.for_class(new_socket_class),
        current_task.into(),
    )
}

/// Sets the peer security context for each socket in the pair.
pub(in crate::security) fn socket_socketpair(
    left: DowncastedFile<'_, SocketFile>,
    right: DowncastedFile<'_, SocketFile>,
) -> Result<(), Errno> {
    let left_sid = fs_node_effective_sid_and_class(left.file().node()).sid;
    let right_sid = fs_node_effective_sid_and_class(right.file().node()).sid;
    *left.socket().security.state.peer_sid.lock() = Some(right_sid);
    *right.socket().security.state.peer_sid.lock() = Some(left_sid);
    Ok(())
}

/// Computes and sets the security class for `socket`.
pub(in crate::security) fn socket_post_create(security_server: &SecurityServer, socket: &Socket) {
    let socket_node = socket.fs_node().expect("socket_post_create without FsNode");
    socket_node.security_state.0.update_class(
        compute_socket_security_class(
            security_server,
            socket.domain,
            socket.socket_type,
            socket.protocol,
        )
        .into(),
    );
}

/// Checks that `current_task` has the right permissions to perform a bind operation on
/// `socket`.
pub(in crate::security) fn check_socket_bind_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
    _socket_address: &SocketAddress,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(TODO("https://fxbug.dev/414583985"), "check_socket_bind_access without FsNode");
        return Ok(());
    };

    let current_sid = current_task_state(current_task).current_sid;

    // TODO: https://fxbug.dev/364569010 - Add checks for `name_bind` between the socket and the SID
    // of the port number, and for `node_bind` between the socket and the SID of the IP address.
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonSocketPermission::Bind,
        current_task.into(),
    )
}

/// Checks that `current_task` has the right permissions to initiate a connection with
/// `socket`.
pub(in crate::security) fn check_socket_connect_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: DowncastedFile<'_, SocketFile>,
    _socket_peer: &SocketPeer,
) -> Result<(), Errno> {
    let current_sid = current_task_state(current_task).current_sid;

    // TODO: https://fxbug.dev/364568577 - Add checks for `name_connect` between the socket and the
    // SID of the port number for TCP sockets.
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket.file().node(),
        CommonSocketPermission::Connect,
        current_task.into(),
    )
}

/// Checks that `current_task` has permission to listen on `socket`.
pub(in crate::security) fn check_socket_listen_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
    _backlog: i32,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_listen_access without FsNode"
        );
        return Ok(());
    };

    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonSocketPermission::Listen,
        current_task.into(),
    )
}

/// Checks that `current_task` has permission to accept a connection on `listening_socket`, and
/// sets the security state for `accepted_socket` to match the context of `listening_socket`.
pub(in crate::security) fn socket_accept(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    listening_socket: DowncastedFile<'_, SocketFile>,
    accepted_socket: DowncastedFile<'_, SocketFile>,
) -> Result<(), Errno> {
    let current_sid = current_task_state(current_task).current_sid;
    let listening_security_state =
        (*listening_socket.file().node().security_state.0.read()).clone();
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &listening_socket.file().node(),
        CommonSocketPermission::Accept,
        current_task.into(),
    )?;
    let _lock = accepted_socket.file().node().security_state.0.update_lock.lock();
    accepted_socket.file().node().security_state.0.label.update(listening_security_state);
    Ok(())
}

/// Checks that `current_task` has permission to get socket options on `socket`.
pub(in crate::security) fn check_socket_getsockopt_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
    level: u32,
    optname: u32,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_getsockopt_access without FsNode"
        );
        return Ok(());
    };

    let audit_context = &[current_task.into(), Auditable::SockOptArguments(level, optname)];
    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonSocketPermission::GetOpt,
        audit_context.into(),
    )
}

/// Checks that `current_task` has permission to set socket options on `socket`.
pub(in crate::security) fn check_socket_setsockopt_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
    _level: u32,
    _optname: u32,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_setsockopt_access without FsNode"
        );
        return Ok(());
    };
    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonSocketPermission::SetOpt,
        current_task.into(),
    )
}

/// Checks that `current_task` has permission to send a message on `socket`.
pub(in crate::security) fn check_socket_sendmsg_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_sendmsg_access without FsNode"
        );
        return Ok(());
    };
    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonFsNodePermission::Write,
        current_task.into(),
    )
}

/// Checks that `current_task` has permission to receive a message on `socket`.
pub(in crate::security) fn check_socket_recvmsg_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_recvmsg_access without FsNode"
        );
        return Ok(());
    };
    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonFsNodePermission::Read,
        current_task.into(),
    )
}

/// Checks that `current_task` has permission to get the name of `socket`.
pub(in crate::security) fn check_socket_getname_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_getname_access without FsNode"
        );
        return Ok(());
    };

    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonFsNodePermission::GetAttr,
        current_task.into(),
    )
}

/// Checks that `current_task` has permission to shutdown `socket`.
pub(in crate::security) fn check_socket_shutdown_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
    _how: SocketShutdownFlags,
) -> Result<(), Errno> {
    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_socket_shutdown_access without FsNode"
        );
        return Ok(());
    };

    let current_sid = current_task_state(current_task).current_sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        CommonSocketPermission::Shutdown,
        current_task.into(),
    )
}

/// Returns the Security Context with which the [`crate::vfs::Socket`]'s peer is labeled.
pub(in crate::security) fn socket_getpeersec_stream(
    security_server: &SecurityServer,
    _current_task: &CurrentTask,
    socket: &Socket,
) -> Result<Vec<u8>, Errno> {
    let peer_sid = socket.security.state.peer_sid.lock().unwrap_or(InitialSid::Unlabeled.into());
    // The SELinux Test Suite assumes that `SO_PEERSEC` will return a NUL terminated label.
    Ok(security_server.sid_to_security_context_with_nul(peer_sid).unwrap())
}

/// Returns the Security Context with which messages sent by this [`crate::vfs::Socket`] should
/// be labeled.
pub(in crate::security) fn socket_getpeersec_dgram(
    security_server: &SecurityServer,
    _current_task: &CurrentTask,
    socket: &Socket,
) -> Vec<u8> {
    let socket_sid = if let Some(socket_node) = socket.fs_node() {
        fs_node_effective_sid_and_class(&socket_node).sid
    } else {
        track_stub!(TODO("https://fxbug.dev/414583985"), "socket_getpeersec_dgram without FsNode");
        InitialSid::Unlabeled.into()
    };
    security_server.sid_to_security_context_with_nul(socket_sid).unwrap()
}

/// Checks if the Unix domain `sending_socket` is allowed to send a message to the
/// `receiving_socket`.
pub(in crate::security) fn unix_may_send(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    sending_socket: &Socket,
    receiving_socket: &Socket,
) -> Result<(), Errno> {
    let (Some(sending_node), Some(receiving_node)) =
        (sending_socket.fs_node(), receiving_socket.fs_node())
    else {
        track_stub!(TODO("https://fxbug.dev/414583985"), "unix_may_send without FsNode");
        return Ok(());
    };

    let sending_sid = fs_node_effective_sid_and_class(&sending_node).sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        sending_sid,
        &receiving_node,
        CommonSocketPermission::SendTo,
        current_task.into(),
    )
}

/// Checks if the Unix domain `client_socket` is allowed to connect to `listening_sock`, and
/// initializes security state for the client and server sockets.
pub(in crate::security) fn unix_stream_connect(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    client_socket: &Socket,
    listening_socket: &Socket,
    server_socket: &Socket,
) -> Result<(), Errno> {
    let (Some(client_node), Some(listening_node)) =
        (client_socket.fs_node(), listening_socket.fs_node())
    else {
        track_stub!(TODO("https://fxbug.dev/414583985"), "unix_stream_connect without FsNode");
        return Ok(());
    };

    // Verify whether the `client_socket` has permission to connect to the `listening_socket`.
    let client_sid = fs_node_effective_sid_and_class(&client_node).sid;
    has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        client_sid,
        &listening_node,
        KernelPermission::from(UnixStreamSocketPermission::ConnectTo),
        current_task.into(),
    )?;

    // Permission is granted, so populate the `peer_sid` of the client & server sockets with one
    // another's SIDs, for e.g. `SO_GETPEERSEC` to return.
    // TODO: https://fxbug.dev/414583985 - the `server_socket` does not yet have an associated
    // `FsNode`, nor security label, so the `listening_socket` label must be used for now.
    let listening_sid = fs_node_effective_sid_and_class(&listening_node).sid;
    *client_socket.security.state.peer_sid.lock() = Some(listening_sid);
    *server_socket.security.state.peer_sid.lock() = Some(client_sid);

    Ok(())
}

/// Checks that `current_task` has permission to create a new TUN device.
pub(in crate::security) fn check_tun_dev_create_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
) -> Result<(), Errno> {
    let current_sid = current_task_state(current_task).current_sid;
    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        current_sid,
        CommonFsNodePermission::Create.for_class(SocketClass::TunSocket),
        current_task.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::get_cached_sid;
    use super::*;
    use crate::security::selinux_hooks::testing::{
        mutate_attrs_for_test, spawn_kernel_with_selinux_hooks_test_policy_and_run,
    };
    use crate::vfs::socket::SocketFile;
    use assert_matches::assert_matches;
    use starnix_uapi::errors::EACCES;
    use starnix_uapi::open_flags::OpenFlags;

    #[fuchsia::test]
    async fn socket_post_create() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let task_sid = security_server
                .security_context_to_sid(b"u:object_r:test_socket_create_yes_t:s0".into())
                .expect("invalid security context");
            mutate_attrs_for_test(current_task, |attrs| attrs.current_sid = task_sid);

            let socket_node = SocketFile::new_socket(
                current_task,
                SocketDomain::Unix,
                SocketType::Stream,
                OpenFlags::RDWR,
                SocketProtocol::IP,
                /* kernel_private=*/ false,
            )
            .expect("failed to create socket");

            let socket_label = socket_node.node().security_state.0.read();
            assert_eq!(socket_label.class(), SocketClass::UnixStreamSocket.into());
            assert_eq!(get_cached_sid(socket_node.node()), Some(task_sid));
        })
        .await;
    }

    #[fuchsia::test]
    async fn socket_create_is_allowed() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let task_sid = security_server
                .security_context_to_sid(b"u:object_r:test_socket_create_yes_t:s0".into())
                .expect("invalid security context");
            mutate_attrs_for_test(current_task, |attrs| attrs.current_sid = task_sid);

            assert_matches!(
                SocketFile::new_socket(
                    current_task,
                    SocketDomain::Unix,
                    SocketType::Stream,
                    OpenFlags::RDWR,
                    SocketProtocol::IP,
                    /* kernel_private= */ false,
                ),
                Ok(_)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn socket_create_is_denied() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let task_sid = security_server
                .security_context_to_sid(b"u:object_r:test_socket_create_no_t:s0".into())
                .expect("invalid security context");
            mutate_attrs_for_test(current_task, |attrs| attrs.current_sid = task_sid);

            assert_matches!(SocketFile::new_socket(current_task,
                    SocketDomain::Unix,
                    SocketType::Stream,
                    OpenFlags::RDWR,
                    SocketProtocol::IP,
                    /* kernel_private= */ false,
                ), Err(errno) if errno == EACCES);
        })
        .await;
    }
}
