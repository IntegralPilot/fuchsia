// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! System object context labeling rules parsed from SELinux binary policy.

use selinux_policy_derive::{Parse, Serialize, Validate};

use super::classes::ClassId;
use super::context::Context;
use super::error::{ParseError, SerializeError};
use super::metadata::PolicyVersion;
use super::parser::{Array, ByteArray, PolicyCursor, PolicyWriter};
use super::traits::{Parse, Serialize};

/// Named context pair mapping a filesystem type string to mount and root security [`Context`]s.
///
/// Corresponding SELinux text policy syntax: `fs_con <fs_type> <fs_context> <root_context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct FilesystemContext {
    name: ByteArray,
    fs_context: Context,
    root_context: Context,
}

/// Named context pair mapping a network interface string to device and packet security [`Context`]s.
///
/// Corresponding SELinux text policy syntax: `netifcon <interface_name> <if_context> <packet_context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct NetworkInterfaceContext {
    name: ByteArray,
    if_context: Context,
    msg_context: Context,
}

/// Port specification mapping a protocol and port range to a security [`Context`].
///
/// Corresponding SELinux text policy syntax: `portcon <protocol> <port_low>-<port_high> <context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct PortContext {
    protocol: u32,
    low_port: u32,
    high_port: u32,
    context: Context,
}

/// IPv4 node specification mapping an address and mask to a security [`Context`].
///
/// Corresponding SELinux text policy syntax: `nodecon <ipv4_addr> <netmask> <context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct IPv4NodeContext {
    address: u32,
    mask: u32,
    context: Context,
}

/// Discriminates among the different kinds of `fs_use_*` labeling statements in policy.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Parse, Serialize, Validate)]
#[policy(wire_type = u32)]
pub enum FsUseType {
    Xattr = 1,
    Trans = 2,
    Task = 3,
}

/// Filesystem labeling behavior rule (`fs_use_xattr`, `fs_use_trans`, `fs_use_task`).
///
/// Corresponding SELinux text policy syntax: `fs_use_xattr <fs_type> <context>`,
/// `fs_use_trans <fs_type> <context>`, or `fs_use_task <fs_type> <context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct FsUse {
    behavior: FsUseType,
    name: ByteArray,
    context: Context,
}

impl FsUse {
    /// Returns the filesystem use statement behavior type.
    pub fn behavior(&self) -> FsUseType {
        self.behavior
    }

    /// Returns the filesystem type name bytes (`<fs_type>`).
    pub fn fs_type(&self) -> &[u8] {
        &self.name
    }

    /// Returns the security [`Context`].
    pub fn context(&self) -> &Context {
        &self.context
    }
}

/// IPv6 node specification mapping a 128-bit address and mask to a security [`Context`].
///
/// Corresponding SELinux text policy syntax: `nodecon <ipv6_addr> <netmask> <context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct IPv6NodeContext {
    address: [u32; 4],
    mask: [u32; 4],
    context: Context,
}

/// InfiniBand partition key specification (for policy versions >= [`PolicyVersion::MIN_INFINIBAND`]).
///
/// Corresponding SELinux text policy syntax: `ibpkeycon <subnet_prefix> <pkey_low>-<pkey_high> <context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct InfiniBandPartitionKey {
    low: u32,
    high: u32,
    context: Context,
}

/// InfiniBand end port specification (for policy versions >= [`PolicyVersion::MIN_INFINIBAND`]).
///
/// Corresponding SELinux text policy syntax: `ibendportcon <device_name> <port> <context>`.
#[derive(Debug, Clone, Parse, Serialize, Validate)]
pub struct InfiniBandEndPort {
    name: ByteArray,
    port: u32,
    context: Context,
}

/// Container for system object context labeling statements (`fs_con`, `portcon`, `netifcon`,
/// `nodecon`, `fs_use_*`, `ibpkeycon`, `ibendportcon`).
#[derive(Debug, Clone, Validate)]
pub struct ObjectContexts {
    filesystems: Array<FilesystemContext>,
    ports: Array<PortContext>,
    network_interfaces: Array<NetworkInterfaceContext>,
    ipv4_nodes: Array<IPv4NodeContext>,
    fs_uses: Array<FsUse>,
    ipv6_nodes: Array<IPv6NodeContext>,
    infiniband_partition_keys: Array<InfiniBandPartitionKey>,
    infiniband_end_ports: Array<InfiniBandEndPort>,
}

impl ObjectContexts {
    /// Returns the filesystem labeling statements table.
    pub fn filesystems(&self) -> &[FilesystemContext] {
        &self.filesystems
    }

    /// Returns the network port labeling statements table.
    pub fn ports(&self) -> &[PortContext] {
        &self.ports
    }

    /// Returns the network interface labeling statements table.
    pub fn network_interfaces(&self) -> &[NetworkInterfaceContext] {
        &self.network_interfaces
    }

    /// Returns the IPv4 node labeling statements table.
    pub fn ipv4_nodes(&self) -> &[IPv4NodeContext] {
        &self.ipv4_nodes
    }

    /// Returns the filesystem behavior labeling statements table.
    pub fn fs_uses(&self) -> &[FsUse] {
        &self.fs_uses
    }

    /// Returns the IPv6 node labeling statements table.
    pub fn ipv6_nodes(&self) -> &[IPv6NodeContext] {
        &self.ipv6_nodes
    }

    /// Returns the InfiniBand partition key labeling statements table.
    pub fn infiniband_partition_keys(&self) -> &[InfiniBandPartitionKey] {
        &self.infiniband_partition_keys
    }

    /// Returns the InfiniBand end port labeling statements table.
    pub fn infiniband_end_ports(&self) -> &[InfiniBandEndPort] {
        &self.infiniband_end_ports
    }
}

impl Parse for ObjectContexts {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let filesystems = Array::<FilesystemContext>::parse(cursor)?;
        let ports = Array::<PortContext>::parse(cursor)?;
        let network_interfaces = Array::<NetworkInterfaceContext>::parse(cursor)?;
        let ipv4_nodes = Array::<IPv4NodeContext>::parse(cursor)?;
        let fs_uses = Array::<FsUse>::parse(cursor)?;
        let ipv6_nodes = Array::<IPv6NodeContext>::parse(cursor)?;
        let (infiniband_partition_keys, infiniband_end_ports) =
            if cursor.policy_version() >= PolicyVersion::MIN_INFINIBAND {
                (
                    Array::<InfiniBandPartitionKey>::parse(cursor)?,
                    Array::<InfiniBandEndPort>::parse(cursor)?,
                )
            } else {
                (Array::default(), Array::default())
            };

        Ok(Self {
            filesystems,
            ports,
            network_interfaces,
            ipv4_nodes,
            fs_uses,
            ipv6_nodes,
            infiniband_partition_keys,
            infiniband_end_ports,
        })
    }
}

impl Serialize for ObjectContexts {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        self.filesystems.serialize(writer)?;
        self.ports.serialize(writer)?;
        self.network_interfaces.serialize(writer)?;
        self.ipv4_nodes.serialize(writer)?;
        self.fs_uses.serialize(writer)?;
        self.ipv6_nodes.serialize(writer)?;
        if writer.version() >= PolicyVersion::MIN_INFINIBAND {
            self.infiniband_partition_keys.serialize(writer)?;
            self.infiniband_end_ports.serialize(writer)?;
        }
        Ok(())
    }
}

/// Context rule for a specific path prefix within a generic filesystem (`genfscon`).
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct GenfsConPath {
    partial_path: ByteArray,
    class: Option<ClassId>,
    context: Context,
}

impl GenfsConPath {
    /// Returns the partial path bytes relative to the root of the filesystem.
    pub fn partial_path(&self) -> &[u8] {
        &self.partial_path
    }

    /// Returns the target [`ClassId`] if specified (0 applies to all object classes).
    pub fn class(&self) -> Option<ClassId> {
        self.class
    }

    /// Returns the security [`Context`].
    pub fn context(&self) -> &Context {
        &self.context
    }
}

/// Generic filesystem labeling statement (`genfscon [fs_type] [partial_path] [class] [context]`).
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct GenfsCon {
    fs_type: ByteArray,
    paths: Array<GenfsConPath>,
}

impl GenfsCon {
    /// Returns the filesystem type name bytes (e.g. `b"proc"` or `b"sysfs"`).
    pub fn fs_type(&self) -> &[u8] {
        &self.fs_type
    }

    /// Returns the array of partial path context rules for this filesystem type.
    pub fn paths(&self) -> &[GenfsConPath] {
        &self.paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::traits::PolicyId;
    use crate::new_policy::{NewPolicy, UserId};

    #[test]
    fn test_object_contexts_minimal_policy() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/minimal_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse minimal_policy");
        policy.validate().expect("validate minimal_policy");

        let object_contexts = policy.object_contexts();
        assert!(object_contexts.filesystems().is_empty());
        assert!(object_contexts.ports().is_empty());
        assert!(object_contexts.network_interfaces().is_empty());
        assert!(object_contexts.ipv4_nodes().is_empty());
        assert!(!object_contexts.fs_uses().is_empty());
        assert!(object_contexts.ipv6_nodes().is_empty());
        assert!(object_contexts.infiniband_partition_keys().is_empty());
        assert!(object_contexts.infiniband_end_ports().is_empty());
    }

    #[test]
    fn test_genfscon_parse_and_serialize() {
        let data = [
            // GenfsCon fs_type (ByteArray: len + "sysfs"):
            5, 0, 0, 0, b's', b'y', b's', b'f', b's', // paths count = 1:
            1, 0, 0, 0, // GenfsConPath partial_path (ByteArray: len + "/"):
            1, 0, 0, 0, b'/', // class = 0 (all classes):
            0, 0, 0, 0, // Context:
            1, 0, 0, 0, // user = 1
            1, 0, 0, 0, // role = 1
            1, 0, 0, 0, // type = 1
            // MlsRange:
            1, 0, 0, 0, // levels_count = 1
            1, 0, 0, 0, // sensitivity_low = 1
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // categories count = 0
        ];
        let mut cursor = PolicyCursor::new(&data);
        let genfscon = GenfsCon::parse(&mut cursor).expect("parse GenfsCon");
        assert_eq!(genfscon.fs_type(), b"sysfs");
        assert_eq!(genfscon.paths().len(), 1);
        let path = &genfscon.paths()[0];
        assert_eq!(path.partial_path(), b"/");
        assert_eq!(path.class(), None);
        assert_eq!(path.context().user(), UserId::from_u32(1).unwrap());

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        genfscon.serialize(&mut policy_writer).expect("serialize GenfsCon");
        assert_eq!(writer.as_slice(), &data);
    }
}
