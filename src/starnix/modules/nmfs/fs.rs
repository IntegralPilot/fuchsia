// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file contains an implementation for storing network information in a file system.
//!
//! Each file within `/sys/fs/fuchsia_network_monitor_fs` represents a network and its properties.

use crate::NetworkManager;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use starnix_core::security::check_task_capable;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::fs_args::parse;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_core::vfs::{
    CacheMode, FileOps, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsNode,
    FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr, MemoryDirectoryFile,
};
use starnix_logging::{log_error, log_warn};

use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::{CAP_NET_ADMIN, FsCred};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{FileMode, mode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, error, gid_t, statfs, uid_t};
use std::borrow::Cow;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_policy_socketproxy::{self as fnp_socketproxy, ProtoProperties};

const DEFAULT_NETWORK_FILE_NAME: &str = "default";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct NetworkMessage {
    pub(crate) netid: u32,
    pub(crate) mark: u32,
    pub(crate) handle: u64,
    #[serde(with = "addr_list")]
    pub(crate) dnsv4: Vec<Ipv4Addr>,
    #[serde(with = "addr_list")]
    pub(crate) dnsv6: Vec<Ipv6Addr>,
    #[serde(flatten)]
    pub(crate) versioned_properties: VersionedProperties,
}

#[derive(Clone, Copy, Debug, Deserialize_repr, PartialEq, Serialize_repr)]
#[repr(u8)]
pub(crate) enum TransportType {
    Cellular = 0,
    Wifi = 1,
    Bluetooth = 2,
    Ethernet = 3,
    Vpn = 4,
    WifiAware = 5,
    Lowpan = 6,
}

#[derive(Clone, Copy, Debug, Deserialize_repr, PartialEq, Serialize_repr)]
#[repr(u8)]
pub(crate) enum NetworkCapability {
    Mms = 0,
    Supl = 1,
    Dun = 2,
    Fota = 3,
    Ims = 4,
    Cbs = 5,
    WifiP2p = 6,
    Ia = 7,
    Rcs = 8,
    Xcap = 9,
    Eims = 10,
    NotMetered = 11,
    Internet = 12,
    NotRestricted = 13,
    Trusted = 14,
    NotVpn = 15,
    Validated = 16,
    CaptivePortal = 17,
    NotRoaming = 18,
    Foreground = 19,
    NotCongested = 20,
    NotSuspended = 21,
    OemPaid = 22,
    Mcs = 23,
    PartialConnectivity = 24,
    TemporarilyNotMetered = 25,
    OemPrivate = 26,
    VehicleInternal = 27,
    NotVcnManaged = 28,
    Enterprise = 29,
    Vsim = 30,
    Bip = 31,
    HeadUnit = 32,
    Mmtel = 33,
    PrioritizeLatency = 34,
    PrioritizeBandwidth = 35,
    LocalNetwork = 36,
    NotBandwidthConstrained = 37,
    PrioritizeUnifiedCommunications = 38,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(tag = "version")]
pub(crate) enum VersionedProperties {
    #[default]
    V1,
    V2 {
        #[serde(with = "transport_list")]
        transports: Vec<TransportType>,
        #[serde(with = "capability_list")]
        capabilities: Vec<NetworkCapability>,
        name: String,
        // Whether or not there is a v4/v6 address assigned for
        // the underlying link properties.
        addrv4: bool,
        addrv6: bool,
        // Whether or not there is a v4/v6 default route for
        // the underlying link properties.
        defaultv4: bool,
        defaultv6: bool,
    },
}

pub struct FuchsiaNetworkMonitorFs;
impl FuchsiaNetworkMonitorFs {
    pub fn new_fs(kernel: &Kernel, options: FileSystemOptions) -> Result<FileSystemHandle, Errno> {
        let mode = if let Some(mode) = options.params.get(b"mode") {
            FileMode::from_string(mode.as_ref())?.with_type(FileMode::IFDIR)
        } else {
            mode!(IFDIR, 0o600)
        };
        let uid = options.params.get_as::<uid_t>(b"uid")?.unwrap_or(0);
        let gid = options.params.get_as::<gid_t>(b"gid")?.unwrap_or(0);

        let fs = FileSystem::new(kernel, CacheMode::Permanent, FuchsiaNetworkMonitorFs, options)?;
        let root_ino = fs.allocate_ino();
        let info = FsNodeInfo::new(mode, FsCred { uid, gid });
        fs.create_root_with_info(root_ino, NetworkDirectoryNode::new(), info);
        Ok(fs)
    }
}

const FUCHSIA_NETWORK_MONITOR_FS_NAME: &[u8; 26] = b"fuchsia_network_monitor_fs";
const FUCHSIA_NETWORK_MONITOR_FS_MAGIC: u32 = u32::from_be_bytes(*b"nmfs");

impl FileSystemOps for FuchsiaNetworkMonitorFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(FUCHSIA_NETWORK_MONITOR_FS_MAGIC))
    }

    fn name(&self) -> &'static FsStr {
        FUCHSIA_NETWORK_MONITOR_FS_NAME.into()
    }
}

pub fn fuchsia_network_monitor_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    struct FuchsiaNetworkMonitorFsHandle(FileSystemHandle);

    let kernel = current_task.kernel();
    Ok(kernel
        .expando
        .get_or_try_init(|| {
            FuchsiaNetworkMonitorFs::new_fs(kernel, options)
                .map(|fs| FuchsiaNetworkMonitorFsHandle(fs))
        })?
        .0
        .clone())
}

// Get the NetworkManager from the Kernel's Expando.
//
// Returns the NetworkManager when available, or EPERM when the NetworkManager
// is absent.
fn try_acquire_network_manager(current_task: &CurrentTask) -> Result<Arc<NetworkManager>, Errno> {
    let kernel = current_task.kernel();
    kernel.expando.peek::<NetworkManager>().ok_or_else(|| errno!(EPERM))
}

pub struct NetworkDirectoryNode;

impl NetworkDirectoryNode {
    pub fn new() -> Self {
        Self
    }
}

impl FsNodeOps for NetworkDirectoryNode {
    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(MemoryDirectoryFile::new()))
    }

    fn mkdir(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _mode: FileMode,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EPERM)
    }

    fn mknod(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
        mode: FileMode,
        _dev: DeviceId,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        check_task_capable(current_task, CAP_NET_ADMIN)?;
        if !mode.is_reg() {
            return error!(EACCES);
        }

        let ops: Box<dyn FsNodeOps> = if name == DEFAULT_NETWORK_FILE_NAME {
            // The node with DEFAULT_NETWORK_FILE_NAME is special and can
            // only be written to with network ids.
            Box::new(DefaultNetworkIdFile::new_node())
        } else {
            let id: u32 = parse(name).map_err(|_| errno!(EINVAL))?;
            // Insert a new network entry, but don't populate any fields.
            let network_manager = try_acquire_network_manager(current_task)?;
            // This call should only occur on the first node with this name,
            // so this call isn't expected to fail.
            network_manager.add_empty_network(id)?;
            Box::new(NetworkFile::new_node(id))
        };

        let child = node.fs().create_node_and_allocate_node_id(
            ops,
            FsNodeInfo::new(mode, current_task.current_fscred()),
        );

        Ok(child)
    }

    fn unlink(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
        _child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        check_task_capable(current_task, CAP_NET_ADMIN)?;
        let network_manager = try_acquire_network_manager(current_task)?;
        // Note: direct equality comparisons are easier using FsStr
        // than using a match block.
        if name == DEFAULT_NETWORK_FILE_NAME {
            // Reset the default network when the associated
            // network file with the same id is unlinked.
            network_manager.set_default_network_id(None);
        } else {
            let id: u32 = parse(name)?;
            network_manager.remove_network(id)?;
        }

        Ok(())
    }

    fn create_symlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _target: &FsStr,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EPERM)
    }
}

pub struct NetworkFile {
    network_id: u32,
}

impl NetworkFile {
    pub fn new_node(network_id: u32) -> impl FsNodeOps {
        BytesFile::new_node(Self { network_id })
    }
}

impl BytesFileOps for NetworkFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        check_task_capable(current_task, CAP_NET_ADMIN)?;
        let network: NetworkMessage = serde_json::from_slice(&data).map_err(|e| {
            log_error!("failed to deserialize network message: {}", e);
            errno!(EINVAL)
        })?;

        let new_netid = network.netid;

        // The network id must be the same as the id listed in the JSON.
        if new_netid != self.network_id {
            return error!(EINVAL);
        }

        let network_manager = try_acquire_network_manager(current_task)?;
        match network_manager.get_network(&new_netid) {
            None | Some(None) => {
                network_manager.add_network(network)?;
            }
            Some(Some(_old_network)) => {
                network_manager.update_network(network)?;
            }
        }

        Ok(())
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let network_manager = try_acquire_network_manager(current_task)?;
        // Verify whether the network exists before reading.
        if let None = network_manager.get_network(&self.network_id) {
            return error!(ENOENT);
        }

        Ok(network_manager.get_network_by_id_as_bytes(self.network_id).to_vec().into())
    }
}

pub struct DefaultNetworkIdFile {}

impl DefaultNetworkIdFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for DefaultNetworkIdFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        check_task_capable(current_task, CAP_NET_ADMIN)?;
        let id_string = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let id: u32 = id_string.parse().map_err(|_| errno!(EINVAL))?;

        {
            let network_manager = try_acquire_network_manager(current_task)?;
            match network_manager.get_network(&id) {
                // A network with the provided id must already
                // exist to become the default network.
                Some(Some(_)) => {
                    network_manager.set_default_network_id(Some(id));
                }
                // The network properties must be provided for
                // a network before it can become the default.
                Some(None) | None => return error!(ENOENT),
            };
        }
        Ok(())
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let network_manager = try_acquire_network_manager(current_task)?;
        if let None = network_manager.get_default_network_id() {
            return error!(ENOENT);
        }

        Ok(network_manager.get_default_id_as_bytes().to_vec().into())
    }
}

impl From<&NetworkMessage> for fnp_socketproxy::Network {
    fn from(message: &NetworkMessage) -> Self {
        let (transports, capabilities, name, addrv4, addrv6, defaultv4, defaultv6) =
            match &message.versioned_properties {
                VersionedProperties::V1 => (None, None, None, None, None, None, None),
                VersionedProperties::V2 {
                    transports,
                    capabilities,
                    name,
                    addrv4,
                    addrv6,
                    defaultv4,
                    defaultv6,
                } => (
                    Some(transports),
                    Some(capabilities),
                    Some(name),
                    Some(addrv4),
                    Some(addrv6),
                    Some(defaultv4),
                    Some(defaultv6),
                ),
            };

        Self {
            network_id: Some(message.netid),
            info: Some(fnp_socketproxy::NetworkInfo::Starnix(
                fnp_socketproxy::StarnixNetworkInfo {
                    mark: Some(message.mark),
                    handle: Some(message.handle),
                    ..Default::default()
                },
            )),
            dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
                v4: Some(
                    message
                        .dnsv4
                        .clone()
                        .into_iter()
                        .map(|a| fnet::Ipv4Address { addr: a.octets() })
                        .collect::<Vec<_>>(),
                ),
                v6: Some(
                    message
                        .dnsv6
                        .clone()
                        .into_iter()
                        .map(|a| fnet::Ipv6Address { addr: a.octets() })
                        .collect::<Vec<_>>(),
                ),
                ..Default::default()
            }),
            network_type: transports
                .map(|transport_list| consolidate_transport_types_to_network_type(transport_list)),
            capabilities: capabilities.map(|capability_list| convert_capabilities(capability_list)),
            connectivity: capabilities
                .map(|capability_list| convert_connectivity_state(capability_list)),
            name: name.cloned(),
            has_address: Some(proto_properties_from_v4_v6(addrv4.copied(), addrv6.copied())),
            has_default_route: Some(proto_properties_from_v4_v6(
                defaultv4.copied(),
                defaultv6.copied(),
            )),
            ..Default::default()
        }
    }
}

fn proto_properties_from_v4_v6(v4: Option<bool>, v6: Option<bool>) -> ProtoProperties {
    let mut properties = ProtoProperties::empty();

    if Some(true) == v4 {
        properties |= ProtoProperties::V4;
    }

    if Some(true) == v6 {
        properties |= ProtoProperties::V6;
    }
    properties
}

// Given a list of `TransportType`, convert it to a single `NetworkType`. Only a single
// transport is expected, so take the first one if multiple are present. If none are
// present, Unknown is used as a fallback.
fn consolidate_transport_types_to_network_type(
    value: &Vec<TransportType>,
) -> fnp_socketproxy::NetworkType {
    value
        .iter()
        .filter_map(|tt| maybe_network_type_from_transport_type(*tt))
        .next()
        .unwrap_or(fnp_socketproxy::NetworkType::Unknown)
}

fn maybe_network_type_from_transport_type(
    value: TransportType,
) -> Option<fnp_socketproxy::NetworkType> {
    match value {
        TransportType::Ethernet => Some(fnp_socketproxy::NetworkType::Ethernet),
        TransportType::Wifi => Some(fnp_socketproxy::NetworkType::Wifi),
        TransportType::Bluetooth => Some(fnp_socketproxy::NetworkType::Bluetooth),
        TransportType::Cellular => Some(fnp_socketproxy::NetworkType::Cellular),
        TransportType::Vpn | TransportType::WifiAware | TransportType::Lowpan => {
            log_warn!("Known NetworkType {value:?} is not currently supported");
            None
        }
    }
}

// Currently, we only check for the following capabilities: NotMetered, NotCongested,
// NotBandwidthConstrained. If we wish to have more knowledge of the offered capabilities,
// we can consider adding fields to the fnp_socketproxy::NetworkRegistry FIDL API.
fn convert_capabilities(value: &Vec<NetworkCapability>) -> Vec<fnp_socketproxy::NetworkCapability> {
    value
        .iter()
        .filter_map(|c| match c {
            NetworkCapability::NotMetered => Some(fnp_socketproxy::NetworkCapability::UNMETERED),
            NetworkCapability::NotCongested => {
                Some(fnp_socketproxy::NetworkCapability::UNCONGESTED)
            }
            NetworkCapability::NotBandwidthConstrained => {
                Some(fnp_socketproxy::NetworkCapability::NOT_BANDWIDTH_CONSTRAINED)
            }
            _ => None,
        })
        .collect::<Vec<_>>()
}

fn convert_connectivity_state(
    value: &Vec<NetworkCapability>,
) -> fnp_socketproxy::ConnectivityState {
    // Validated and Internet capabilities indicate that the network has been
    // validated to have internet connectivity (HTTP and DNS).
    if value.contains(&NetworkCapability::Validated) && value.contains(&NetworkCapability::Internet)
    {
        return fnp_socketproxy::ConnectivityState::FullConnectivity;
    }

    if value.contains(&NetworkCapability::PartialConnectivity) {
        return fnp_socketproxy::ConnectivityState::PartialConnectivity;
    }

    if value.contains(&NetworkCapability::LocalNetwork) {
        return fnp_socketproxy::ConnectivityState::LocalConnectivity;
    }

    return fnp_socketproxy::ConnectivityState::NoConnectivity;
}

mod addr_list {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

    pub fn serialize<S, Addr>(addr: &Vec<Addr>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        Addr: std::fmt::Display,
    {
        let strings = addr.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        strings.serialize(serializer)
    }

    pub fn deserialize<'de, D, Addr>(deserializer: D) -> Result<Vec<Addr>, D::Error>
    where
        D: Deserializer<'de>,
        Addr: std::str::FromStr,
        Addr::Err: std::fmt::Display,
    {
        let s = Vec::<String>::deserialize(deserializer)?;
        s.into_iter().map(|s| s.parse().map_err(de::Error::custom)).collect()
    }
}

macro_rules! tolerant_repr_serde_impl {
    ($module_name:ident, $NewType:path) => {
        mod $module_name {
            use serde::{Deserialize, Deserializer, Serialize, Serializer};
            use starnix_logging::log_error;

            pub fn serialize<S>(items: &Vec<$NewType>, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let nums = items.iter().map(|item| *item as u8).collect::<Vec<_>>();
                nums.serialize(serializer)
            }

            pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<$NewType>, D::Error>
            where
                D: Deserializer<'de>,
            {
                let vals = Vec::<serde_json::Value>::deserialize(deserializer)?;
                Ok(vals
                    .into_iter()
                    .filter_map(|val| match serde_json::from_value::<$NewType>(val.clone()) {
                        Ok(s) => Some(s),
                        Err(_) => {
                            log_error!("unknown value: {}", val);
                            None
                        }
                    })
                    .collect())
            }
        }
    };
}

tolerant_repr_serde_impl!(transport_list, crate::TransportType);
tolerant_repr_serde_impl!(capability_list, crate::NetworkCapability);

#[cfg(test)]
mod tests {
    use super::*;
    use net_declare::{std_ip_v4, std_ip_v6};
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_core::vfs::fs_args::MountParams;
    use test_case::test_case;

    #[::fuchsia::test]
    fn network_message_serde() {
        let network = NetworkMessage {
            netid: 123,
            mark: 456,
            handle: 789,
            dnsv4: vec![std_ip_v4!("192.168.0.1")],
            dnsv6: vec![std_ip_v6!("2001:db8::1")],
            versioned_properties: VersionedProperties::V1,
        };
        serde_helper(network);
    }

    #[::fuchsia::test]
    fn network_message_serde_version2() {
        let network = NetworkMessage {
            netid: 123,
            mark: 456,
            handle: 789,
            dnsv4: vec![std_ip_v4!("192.168.0.1")],
            dnsv6: vec![std_ip_v6!("2001:db8::1")],
            versioned_properties: VersionedProperties::V2 {
                transports: vec![
                    TransportType::Cellular,
                    TransportType::Wifi,
                    TransportType::Bluetooth,
                    TransportType::Ethernet,
                    TransportType::Vpn,
                    TransportType::WifiAware,
                    TransportType::Lowpan,
                ],
                capabilities: vec![
                    NetworkCapability::Trusted,
                    NetworkCapability::Internet,
                    NetworkCapability::Validated,
                ],
                name: "test01".to_string(),
                addrv4: false,
                addrv6: true,
                defaultv4: false,
                defaultv6: true,
            },
        };
        serde_helper(network);
    }

    #[::fuchsia::test]
    fn network_message_serde_unknown_transports_and_capabilities() {
        let json = r#"{
            "netid": 123,
            "mark": 456,
            "handle": 789,
            "dnsv4": ["192.168.0.1"],
            "dnsv6": ["2001:db8::1"],
            "version": "V2",
            "transports": [1, 99, 3],
            "capabilities": [11, 99],
            "name": "test01",
            "addrv4": false,
            "addrv6": false,
            "defaultv4": false,
            "defaultv6": false
        }"#;

        let expected = NetworkMessage {
            netid: 123,
            mark: 456,
            handle: 789,
            dnsv4: vec![std_ip_v4!("192.168.0.1")],
            dnsv6: vec![std_ip_v6!("2001:db8::1")],
            versioned_properties: VersionedProperties::V2 {
                transports: vec![TransportType::Wifi, TransportType::Ethernet],
                capabilities: vec![NetworkCapability::NotMetered],
                name: "test01".to_string(),
                addrv4: false,
                addrv6: false,
                defaultv4: false,
                defaultv6: false,
            },
        };

        let deserialized: NetworkMessage = serde_json::from_str(json).unwrap();
        assert_eq!(expected, deserialized);
    }

    // Ensure that a provided NetworkMessage can be serialized and
    // deserialized into the original form.
    fn serde_helper(network: NetworkMessage) {
        let serialized = serde_json::to_string(&network).unwrap_or_else(|_| "{}".to_string());
        let deserialized = serde_json::from_str(&serialized).unwrap();
        assert_eq!(network, deserialized);
    }

    #[test_case(vec![TransportType::Cellular], fnp_socketproxy::NetworkType::Cellular;
        "cellular")]
    #[test_case(vec![TransportType::Wifi], fnp_socketproxy::NetworkType::Wifi;
        "wifi")]
    #[test_case(vec![TransportType::Bluetooth], fnp_socketproxy::NetworkType::Bluetooth;
        "bluetooth")]
    #[test_case(vec![TransportType::Ethernet], fnp_socketproxy::NetworkType::Ethernet;
        "ethernet")]
    #[test_case(
        vec![
            TransportType::Wifi,
            TransportType::Cellular
        ],
        fnp_socketproxy::NetworkType::Wifi; "first_transport_prioritized")]
    #[test_case(vec![], fnp_socketproxy::NetworkType::Unknown; "empty_fallback")]
    #[test_case(
        vec![
            TransportType::Lowpan,
            TransportType::WifiAware,
            TransportType::Vpn
        ],
        fnp_socketproxy::NetworkType::Unknown; "none_applicable")]
    #[::fuchsia::test]
    fn test_consolidate_transport_types_to_network_type(
        transports: Vec<TransportType>,
        expected: fnp_socketproxy::NetworkType,
    ) {
        assert_eq!(consolidate_transport_types_to_network_type(&transports), expected);
    }

    #[test_case(
        vec![
            NetworkCapability::NotMetered,
            NetworkCapability::NotCongested,
            NetworkCapability::NotBandwidthConstrained,
        ],
        vec![
            fnp_socketproxy::NetworkCapability::UNMETERED,
            fnp_socketproxy::NetworkCapability::UNCONGESTED,
            fnp_socketproxy::NetworkCapability::NOT_BANDWIDTH_CONSTRAINED,
        ];
        "all_capabilities"
    )]
    #[test_case(
        vec![NetworkCapability::NotMetered],
        vec![fnp_socketproxy::NetworkCapability::UNMETERED];
        "one_capability"
    )]
    #[test_case(
        vec![],
        vec![];
        "no_capabilities"
    )]
    #[test_case(
        vec![
            NetworkCapability::NotMetered,
            NetworkCapability::Trusted, // This should be ignored.
        ],
        vec![fnp_socketproxy::NetworkCapability::UNMETERED];
        "ignore_unrelated"
    )]
    #[::fuchsia::test]
    fn test_convert_capabilities(
        capabilities: Vec<NetworkCapability>,
        expected: Vec<fnp_socketproxy::NetworkCapability>,
    ) {
        let mut result = convert_capabilities(&capabilities);
        result.sort();
        let mut expected_sorted = expected;
        expected_sorted.sort();
        assert_eq!(result, expected_sorted);
    }

    #[test_case(
        vec![
            NetworkCapability::Validated,
            NetworkCapability::Internet,
        ],
        fnp_socketproxy::ConnectivityState::FullConnectivity;
        "full_connectivity"
    )]
    #[test_case(
        vec![
            NetworkCapability::Validated,
            NetworkCapability::Internet,
            NetworkCapability::LocalNetwork,
        ],
        fnp_socketproxy::ConnectivityState::FullConnectivity;
        "full_connectivity_prioritized"
    )]
    #[test_case(
        vec![NetworkCapability::PartialConnectivity],
        fnp_socketproxy::ConnectivityState::PartialConnectivity;
        "partial_connectivity"
    )]
    #[test_case(
        vec![NetworkCapability::LocalNetwork],
        fnp_socketproxy::ConnectivityState::LocalConnectivity;
        "local_connectivity"
    )]
    #[test_case(
        vec![],
        fnp_socketproxy::ConnectivityState::NoConnectivity;
        "no_connectivity"
    )]
    #[test_case(
        vec![NetworkCapability::Trusted], // This should be ignored.
        fnp_socketproxy::ConnectivityState::NoConnectivity;
        "ignore_unrelated"
    )]
    #[::fuchsia::test]
    fn test_convert_connectivity_state(
        capabilities: Vec<NetworkCapability>,
        expected: fnp_socketproxy::ConnectivityState,
    ) {
        assert_eq!(convert_connectivity_state(&capabilities), expected);
    }

    #[::fuchsia::test]
    async fn test_mode_option() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = FuchsiaNetworkMonitorFs::new_fs(
                &kernel,
                FileSystemOptions {
                    params: MountParams::parse(b"mode=0123,uid=1000,gid=2000".into())
                        .expect("parsed correctly"),
                    ..Default::default()
                },
            )
            .expect("new_fs");
            {
                let info = fs.root().node.info();
                assert_eq!(info.mode, mode!(IFDIR, 0o123));
                assert_eq!(info.uid, 1000);
                assert_eq!(info.gid, 2000);
            }

            // Test defaults.
            let fs = FuchsiaNetworkMonitorFs::new_fs(&kernel, FileSystemOptions::default())
                .expect("new_fs");
            {
                let info = fs.root().node.info();
                assert_eq!(info.mode, mode!(IFDIR, 0o600));
                assert_eq!(info.uid, 0);
                assert_eq!(info.gid, 0);
            }
        })
        .await;
    }
}
