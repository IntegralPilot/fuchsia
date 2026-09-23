// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security::Auditable;
use crate::security::selinux_hooks::{
    KernelPermission, build_permission_check, check_permission_and_xperms, current_task_state,
    fs_node_effective_sid_and_class, socket,
};
use crate::task::CurrentTask;
use crate::vfs::socket::{NetlinkFamily, Socket, SocketDomain};
use linux_uapi::{
    AUDIT_ADD_RULE, AUDIT_DEL_RULE, AUDIT_FIRST_USER_MSG, AUDIT_FIRST_USER_MSG2, AUDIT_GET,
    AUDIT_GET_FEATURE, AUDIT_LAST_USER_MSG, AUDIT_LAST_USER_MSG2, AUDIT_LIST_RULES, AUDIT_SET,
    AUDIT_SET_FEATURE, AUDIT_TTY_SET, AUDIT_USER, RTM_DELADDR, RTM_DELCHAIN, RTM_DELLINK,
    RTM_DELLINKPROP, RTM_DELNEIGH, RTM_DELNSID, RTM_DELQDISC, RTM_DELROUTE, RTM_DELRULE,
    RTM_DELTCLASS, RTM_DELTFILTER, RTM_GETADDR, RTM_GETCHAIN, RTM_GETLINK, RTM_GETNEIGH,
    RTM_GETNEIGHTBL, RTM_GETNSID, RTM_GETQDISC, RTM_GETROUTE, RTM_GETRULE, RTM_GETTCLASS,
    RTM_GETTFILTER, RTM_NEWADDR, RTM_NEWCHAIN, RTM_NEWLINK, RTM_NEWLINKPROP, RTM_NEWNDUSEROPT,
    RTM_NEWNEIGH, RTM_NEWNEIGHTBL, RTM_NEWNSID, RTM_NEWPREFIX, RTM_NEWQDISC, RTM_NEWROUTE,
    RTM_NEWRULE, RTM_NEWTCLASS, RTM_NEWTFILTER, RTM_SETLINK, RTM_SETNEIGHTBL, SOCK_DESTROY,
    SOCK_DIAG_BY_FAMILY, XFRM_MSG_DELPOLICY, XFRM_MSG_DELSA, XFRM_MSG_GETPOLICY, XFRM_MSG_GETSA,
    XFRM_MSG_NEWPOLICY, XFRM_MSG_NEWSA,
};
use selinux::policy::XpermsKind;
use selinux::{
    NetlinkAuditSocketPermission, NetlinkRouteSocketPermission, NetlinkTcpDiagSocketPermission,
    NetlinkXfrmSocketPermission, PolicyCap, SecurityServer,
};
use starnix_logging::track_stub;
use starnix_uapi::errors::Errno;

/// Returns the "nlmsg" `KernelPermission` for the specified `NetlinkFamily`.
fn nlmsg_permission_for_netlink_family(netlink_family: NetlinkFamily) -> Option<KernelPermission> {
    match netlink_family {
        NetlinkFamily::Route => Some(NetlinkRouteSocketPermission::Nlmsg.into()),
        NetlinkFamily::Audit => Some(NetlinkAuditSocketPermission::Nlmsg.into()),
        NetlinkFamily::SockDiag => Some(NetlinkTcpDiagSocketPermission::Nlmsg.into()),
        NetlinkFamily::Xfrm => Some(NetlinkXfrmSocketPermission::Nlmsg.into()),

        // Other Netlink families don't have the "nlmsg" permission.
        NetlinkFamily::Invalid
        | NetlinkFamily::Selinux
        | NetlinkFamily::Nflog
        | NetlinkFamily::Usersock
        | NetlinkFamily::Iscsi
        | NetlinkFamily::FibLookup
        | NetlinkFamily::Firewall
        | NetlinkFamily::Connector
        | NetlinkFamily::Rdma
        | NetlinkFamily::Crypto
        | NetlinkFamily::Smc
        | NetlinkFamily::Netfilter
        | NetlinkFamily::Ip6Fw
        | NetlinkFamily::Dnrtmsg
        | NetlinkFamily::KobjectUevent
        | NetlinkFamily::Generic
        | NetlinkFamily::Scsitransport
        | NetlinkFamily::Ecryptfs => None,
    }
}

/// Computes the required legacy Netlink permission for `netlink_family` and `message_type`.
fn legacy_permission_for_family_and_message_type(
    netlink_family: NetlinkFamily,
    message_type: u16,
) -> Option<KernelPermission> {
    match netlink_family {
        NetlinkFamily::Route => match message_type as u32 {
            RTM_GETNEIGH => Some(NetlinkRouteSocketPermission::NlmsgGetNeigh.into()),
            RTM_GETROUTE | RTM_GETADDR | RTM_GETNEIGHTBL | RTM_GETQDISC | RTM_GETCHAIN
            | RTM_GETNSID | RTM_GETRULE | RTM_GETTCLASS | RTM_GETTFILTER => {
                Some(NetlinkRouteSocketPermission::NlmsgRead.into())
            }
            RTM_GETLINK => Some(NetlinkRouteSocketPermission::NlmsgReadPriv.into()),
            RTM_NEWROUTE | RTM_DELROUTE | RTM_NEWLINK | RTM_DELLINK | RTM_NEWADDR | RTM_DELADDR
            | RTM_DELCHAIN | RTM_DELLINKPROP | RTM_DELNEIGH | RTM_DELNSID | RTM_DELQDISC
            | RTM_DELRULE | RTM_DELTCLASS | RTM_DELTFILTER | RTM_NEWCHAIN | RTM_NEWLINKPROP
            | RTM_NEWNDUSEROPT | RTM_NEWNEIGH | RTM_NEWNEIGHTBL | RTM_NEWNSID | RTM_NEWPREFIX
            | RTM_NEWQDISC | RTM_NEWRULE | RTM_NEWTCLASS | RTM_NEWTFILTER | RTM_SETLINK
            | RTM_SETNEIGHTBL => Some(NetlinkRouteSocketPermission::NlmsgWrite.into()),
            _ => None,
        },
        NetlinkFamily::Audit => match message_type as u32 {
            AUDIT_GET | AUDIT_GET_FEATURE => Some(NetlinkAuditSocketPermission::NlmsgRead.into()),
            AUDIT_LIST_RULES => Some(NetlinkAuditSocketPermission::NlmsgReadPriv.into()),
            AUDIT_USER
            | AUDIT_FIRST_USER_MSG..=AUDIT_LAST_USER_MSG
            | AUDIT_FIRST_USER_MSG2..=AUDIT_LAST_USER_MSG2 => {
                Some(NetlinkAuditSocketPermission::NlmsgRelay.into())
            }
            AUDIT_TTY_SET => Some(NetlinkAuditSocketPermission::NlmsgTtyAudit.into()),
            AUDIT_SET | AUDIT_SET_FEATURE | AUDIT_ADD_RULE | AUDIT_DEL_RULE => {
                Some(NetlinkAuditSocketPermission::NlmsgWrite.into())
            }
            _ => None,
        },
        NetlinkFamily::SockDiag => match message_type as u32 {
            SOCK_DIAG_BY_FAMILY => Some(NetlinkTcpDiagSocketPermission::NlmsgRead.into()),
            SOCK_DESTROY => Some(NetlinkTcpDiagSocketPermission::NlmsgWrite.into()),
            _ => None,
        },
        NetlinkFamily::Xfrm => match message_type as u32 {
            XFRM_MSG_GETSA | XFRM_MSG_GETPOLICY => {
                Some(NetlinkXfrmSocketPermission::NlmsgRead.into())
            }
            XFRM_MSG_NEWSA | XFRM_MSG_DELSA | XFRM_MSG_NEWPOLICY | XFRM_MSG_DELPOLICY => {
                Some(NetlinkXfrmSocketPermission::NlmsgWrite.into())
            }
            _ => None,
        },

        // Some families define "nlmsg_read" and "nlmsg_write" but are not currently supported
        // in Starnix.
        NetlinkFamily::Firewall | NetlinkFamily::Ip6Fw => None,

        // Other Netlink families don't have message permissions besides the common socket
        // permissions.
        NetlinkFamily::Invalid
        | NetlinkFamily::Selinux
        | NetlinkFamily::Nflog
        | NetlinkFamily::Usersock
        | NetlinkFamily::Iscsi
        | NetlinkFamily::FibLookup
        | NetlinkFamily::Connector
        | NetlinkFamily::Rdma
        | NetlinkFamily::Crypto
        | NetlinkFamily::Smc
        | NetlinkFamily::Netfilter
        | NetlinkFamily::Dnrtmsg
        | NetlinkFamily::KobjectUevent
        | NetlinkFamily::Generic
        | NetlinkFamily::Scsitransport
        | NetlinkFamily::Ecryptfs => None,
    }
}

/// Checks if the Netlink `socket` is allowed to send a message of `message_type`.
pub(in crate::security) fn check_netlink_send_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    socket: &Socket,
    message_type: u16,
) -> Result<(), Errno> {
    assert_eq!(
        socket.domain,
        SocketDomain::Netlink,
        "check_netlink_send_access called for non-Netlink socket"
    );

    let Some(socket_node) = socket.fs_node() else {
        track_stub!(
            TODO("https://fxbug.dev/414583985"),
            "check_netlink_send_access called without FsNode"
        );
        return Ok(());
    };

    let netlink_family = NetlinkFamily::from_raw(socket.protocol.as_raw());
    let audit_context = &[current_task.into(), Auditable::NlMsgtype(message_type)];

    if security_server.is_policycap_enabled(PolicyCap::NetlinkXperm) {
        // Permissions are allowed for kernel sockets.
        if socket_node.is_private() {
            return Ok(());
        }

        // If "netlink_xperm" is enabled then make an extended permissions check for "nlmsg".
        let Some(nlmsg_permission) = nlmsg_permission_for_netlink_family(netlink_family) else {
            // No "nlmsg" permission checks are required for this netlink family.
            return Ok(());
        };
        let current_sid = current_task_state(current_task).current_sid;
        let socket_sid = fs_node_effective_sid_and_class(&socket_node).sid;
        let audit_context = &[audit_context.into(), socket_node.as_ref().as_ref().into()];
        return check_permission_and_xperms(
            &build_permission_check(current_task, security_server),
            current_task,
            current_sid,
            socket_sid,
            nlmsg_permission,
            XpermsKind::Nlmsg,
            message_type,
            audit_context.into(),
        );
    }

    let Some(legacy_permission) =
        legacy_permission_for_family_and_message_type(netlink_family, message_type)
    else {
        // No "nlmsg_read"/"nlmsg_write" permission checks are required for this netlink family.
        return Ok(());
    };

    let current_sid = current_task_state(current_task).current_sid;
    socket::has_socket_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        current_sid,
        &socket_node,
        legacy_permission,
        audit_context.into(),
    )
}
