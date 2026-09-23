// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::atomic::Ordering;

use crate::sysctl_directory::SysctlDirectory;
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_settings as fnet_settings;
use fuchsia_component::client::connect_to_protocol_sync;
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
use netlink::{SysctlError, SysctlInterfaceSelector};
use starnix_core::task::CurrentTask;
use starnix_core::vfs::pseudo::simple_file::{
    BytesFile, BytesFileOps, SimpleFileNode, parse_i32_file, serialize_for_file,
};
use starnix_core::vfs::pseudo::stub_bytes_file::StubBytesFile;
use starnix_core::vfs::{
    DirectoryEntryType, DirentSink, FileObject, FileOps, FsNode, FsNodeHandle, FsNodeOps, FsStr,
    emit_dotdot, fileops_impl_directory, fileops_impl_noop_sync, fileops_impl_unbounded_seek,
    fs_node_impl_dir_readonly,
};
use starnix_logging::{bug_ref, log_error, log_warn, track_stub};

use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{FileMode, mode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{errno, error, uapi};
use std::borrow::Cow;

const FILE_MODE: FileMode = mode!(IFREG, 0o644);

fn netstack_devices_readdir(
    file: &FileObject,
    current_task: &CurrentTask,
    sink: &mut dyn DirentSink,
) -> Result<(), Errno> {
    file.blocking_op(current_task, FdEvents::empty(), None, || {
        let (initialized, _) = &current_task.kernel().netstack_devices.initialized_and_wq;
        if !initialized.load(Ordering::SeqCst) {
            // Kick off the initialization of the netlink worker if not yet.
            let _ = current_task.kernel().network_netlink();
            return error!(EAGAIN);
        }
        emit_dotdot(file, sink)?;

        if sink.offset() == 2 {
            sink.add(
                file.fs.allocate_ino(),
                sink.offset() + 1,
                DirectoryEntryType::from_mode(FILE_MODE),
                "all".into(),
            )?;
        }

        if sink.offset() == 3 {
            sink.add(
                file.fs.allocate_ino(),
                sink.offset() + 1,
                DirectoryEntryType::from_mode(FILE_MODE),
                "default".into(),
            )?;
        }

        let devices = current_task.kernel().netstack_devices.snapshot_devices();
        for (name, _) in devices.iter().skip(sink.offset() as usize - 4) {
            let inode_num = file.fs.allocate_ino();
            sink.add(
                inode_num,
                sink.offset() + 1,
                DirectoryEntryType::from_mode(FILE_MODE),
                name.as_ref(),
            )?;
        }
        Ok(())
    })
}

macro_rules! fileops_impl_netstack_devices {
    () => {
        fn readdir(
            &self,
            file: &FileObject,
            current_task: &CurrentTask,
            sink: &mut dyn DirentSink,
        ) -> Result<(), Errno> {
            netstack_devices_readdir(file, current_task, sink)
        }

        fn wait_async(
            &self,
            _file: &FileObject,
            current_task: &CurrentTask,
            waiter: &starnix_core::task::Waiter,
            _events: FdEvents,
            _handler: starnix_core::task::EventHandler,
        ) -> Option<starnix_core::task::WaitCanceler> {
            let (_initialized, wq) = &current_task.kernel().netstack_devices.initialized_and_wq;
            Some(wq.wait_async(waiter))
        }
    };
}

fn get_netstack_device(
    current_task: &CurrentTask,
    name: &FsStr,
) -> Option<SysctlInterfaceSelector> {
    // Kick off the initialization of netlink worker.
    let _ = current_task.kernel().network_netlink();
    // Per https://www.kernel.org/doc/Documentation/networking/ip-sysctl.txt,
    //
    //   conf/default/*:
    //	   Change the interface-specific default settings.
    //
    //   conf/all/*:
    //	   Change all the interface-specific settings.
    //
    // Note that the all/default directories don't exist in `/sys/class/net`.
    if name == "all" {
        return Some(SysctlInterfaceSelector::All);
    }
    if name == "default" {
        return Some(SysctlInterfaceSelector::Default);
    }
    if let Some(dev) = current_task.kernel().netstack_devices.get_device(name) {
        return Some(SysctlInterfaceSelector::Id(dev.interface_id));
    }
    None
}

#[derive(Clone)]
pub struct ProcSysNetIpv4Conf;

impl FsNodeOps for ProcSysNetIpv4Conf {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }

    fn lookup(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if get_netstack_device(current_task, name).is_some() {
            let fs = node.fs();
            let dir = SysctlDirectory::<{ uapi::CAP_NET_ADMIN }>::new();
            dir.edit(&fs, |dir| {
                dir.entry(
                    "accept_redirects",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423646442")),
                    FILE_MODE,
                );
            });
            // TODO: Validate the mode bits are correct.
            return Ok(dir.into_node(&fs, 0o777));
        }
        error!(ENOENT, "looking for {name}")
    }
}

impl FileOps for ProcSysNetIpv4Conf {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();
    fileops_impl_unbounded_seek!();
    fileops_impl_netstack_devices!();
}

#[derive(Clone)]
pub struct ProcSysNetIpv4Neigh;

impl FsNodeOps for ProcSysNetIpv4Neigh {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }

    fn lookup(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if let Some(interface) = get_netstack_device(current_task, name) {
            let fs = node.fs();
            let dir = SysctlDirectory::<{ uapi::CAP_NET_ADMIN }>::new();
            dir.edit(&fs, |dir| {
                dir.entry(
                    "ucast_solicit",
                    new_interface_config_file_node::<UcastSolicit<Ipv4>>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "retrans_time_ms",
                    new_interface_config_file_node::<RetransTimeMs<Ipv4>>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "mcast_resolicit",
                    new_interface_config_file_node::<McastResolicit<Ipv4>>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "base_reachable_time_ms",
                    new_interface_config_file_node::<BaseReachableTimeMs<Ipv4>>(interface),
                    FILE_MODE,
                );
            });
            // TODO: Validate the mode bits are correct.
            return Ok(dir.into_node(&fs, 0o777));
        }
        error!(ENOENT, "looking for {name}")
    }
}

impl FileOps for ProcSysNetIpv4Neigh {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();
    fileops_impl_unbounded_seek!();
    fileops_impl_netstack_devices!();
}

#[derive(Clone)]
pub struct ProcSysNetIpv6Conf;

impl FsNodeOps for ProcSysNetIpv6Conf {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }

    fn lookup(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if let Some(interface) = get_netstack_device(current_task, name) {
            let fs = node.fs();
            let dir = SysctlDirectory::<{ uapi::CAP_NET_ADMIN }>::new();
            dir.edit(&fs, |dir| {
                dir.entry(
                    "accept_ra",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423646365")),
                    FILE_MODE,
                );
                dir.entry(
                    "accept_ra_defrtr",
                    new_interface_config_file_node::<AcceptRaDefrtr>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "accept_ra_info_min_plen",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423645816")),
                    FILE_MODE,
                );
                dir.entry(
                    "accept_ra_rt_info_min_plen",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/322908046")),
                    FILE_MODE,
                );
                dir.entry(
                    "accept_ra_rt_table",
                    NetworkNetlinkSysctlFile::new_node(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "accept_redirects",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423646442")),
                    FILE_MODE,
                );
                dir.entry(
                    "dad_transmits",
                    new_interface_config_file_node::<Ipv6DadTransmits>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "use_tempaddr",
                    new_interface_config_file_node::<UseTempAddr>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "addr_gen_mode",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423645864")),
                    FILE_MODE,
                );
                dir.entry(
                    "stable_secret",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423646722")),
                    FILE_MODE,
                );
                dir.entry(
                    "disable_ipv6",
                    new_interface_config_file_node::<DisableIpv6>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "optimistic_dad",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423646584")),
                    FILE_MODE,
                );
                dir.entry(
                    "use_oif_addrs_only",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423645421")),
                    FILE_MODE,
                );
                dir.entry(
                    "use_optimistic",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/423645883")),
                    FILE_MODE,
                );
                dir.entry(
                    "forwarding",
                    StubBytesFile::new_node(bug_ref!("https://fxbug.dev/322907925")),
                    FILE_MODE,
                );
            });
            // TODO: Validate the mode bits are correct.
            return Ok(dir.into_node(&fs, 0o777));
        }
        error!(ENOENT, "looking for {name}")
    }
}

impl FileOps for ProcSysNetIpv6Conf {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();
    fileops_impl_unbounded_seek!();
    fileops_impl_netstack_devices!();
}

#[derive(Clone)]
pub struct ProcSysNetIpv6Neigh;

impl FsNodeOps for ProcSysNetIpv6Neigh {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }

    fn lookup(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if let Some(interface) = get_netstack_device(current_task, name) {
            let fs = node.fs();
            let dir = SysctlDirectory::<{ uapi::CAP_NET_ADMIN }>::new();
            dir.edit(&fs, |dir| {
                dir.entry(
                    "ucast_solicit",
                    new_interface_config_file_node::<UcastSolicit<Ipv6>>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "retrans_time_ms",
                    new_interface_config_file_node::<RetransTimeMs<Ipv6>>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "mcast_resolicit",
                    new_interface_config_file_node::<McastResolicit<Ipv6>>(interface),
                    FILE_MODE,
                );
                dir.entry(
                    "base_reachable_time_ms",
                    new_interface_config_file_node::<BaseReachableTimeMs<Ipv6>>(interface),
                    FILE_MODE,
                );
            });
            // TODO: Validate the mode bits are correct.
            return Ok(dir.into_node(&fs, 0o777));
        }
        error!(ENOENT, "looking for {name}")
    }
}

impl FileOps for ProcSysNetIpv6Neigh {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();
    fileops_impl_unbounded_seek!();
    fileops_impl_netstack_devices!();
}

struct NetworkNetlinkSysctlFile {
    interface: SysctlInterfaceSelector,
}

impl NetworkNetlinkSysctlFile {
    fn new_node(interface: SysctlInterfaceSelector) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| Ok(BytesFile::new(Self { interface })))
    }
}

fn to_errno(error: SysctlError) -> Errno {
    match error {
        SysctlError::Disconnected => errno!(EIO),
        SysctlError::NoInterface => errno!(ENODEV),
        SysctlError::Unsupported => errno!(ENOTSUP),
    }
}

impl BytesFileOps for NetworkNetlinkSysctlFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let value = parse_i32_file(&data)?;
        current_task
            .kernel()
            .network_netlink()
            .write_accept_ra_rt_table(self.interface, value)
            .map_err(|err| {
                log_error!("failed to write to {:?}: {:?}", self.interface, err);
                to_errno(err)
            })
    }

    fn read(&self, current_task: &CurrentTask) -> Result<std::borrow::Cow<'_, [u8]>, Errno> {
        let value = current_task
            .kernel()
            .network_netlink()
            .read_accept_ra_rt_table(self.interface)
            .map_err(|err| {
                log_error!("failed to read from {:?}: {:?}", self.interface, err);
                to_errno(err)
            })?;
        Ok(serialize_for_file(value).into())
    }
}

pub struct PingGroupRangeFile;

impl PingGroupRangeFile {
    const MAX_GID: u32 = 4294967294;

    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self)
    }
}

impl BytesFileOps for PingGroupRangeFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let mut params = std::str::from_utf8(&data)
            .map_err(|_| errno!(EINVAL))?
            .trim_ascii()
            .split_ascii_whitespace();
        let min = params
            .next()
            .ok_or_else(|| errno!(EINVAL))?
            .parse::<u32>()
            .map_err(|_| errno!(EINVAL))?;
        if min > Self::MAX_GID {
            return error!(EINVAL);
        }

        // Max value is optional.
        let max = match params.next() {
            Some(v) => {
                let v = v.parse::<u32>().map_err(|_| errno!(EINVAL))?;
                if v > Self::MAX_GID {
                    return error!(EINVAL);
                }
                Some(v + 1)
            }
            None => None,
        };

        let mut range = current_task.kernel().system_limits.socket.icmp_ping_gids.lock();
        range.start = min;
        if let Some(max) = max {
            range.end = max;
        }
        if range.is_empty() {
            // Default to "[1, 0]" range (equivalent to "[1, 1)") to match
            // Linux behavior.
            *range = 1..1;
        }

        Ok(())
    }
    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let range = current_task.kernel().system_limits.socket.icmp_ping_gids.lock().clone();
        Ok(format!("{}\t{}\n", range.start, range.end - 1).into_bytes().into())
    }
}

trait InterfaceConfig: Sync + Send + 'static {
    fn try_from_i32(value: i32) -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno>;
    fn try_into_i32(config: fidl_fuchsia_net_interfaces_admin::Configuration)
    -> Result<i32, Errno>;
}

fn new_interface_config_file_node<Config>(selector: SysctlInterfaceSelector) -> impl FsNodeOps
where
    Config: InterfaceConfig,
{
    SimpleFileNode::new(move |_| {
        Ok(BytesFile::new(InterfaceConfigFile {
            selector,
            _marker: std::marker::PhantomData::<Config>,
        }))
    })
}

struct InterfaceConfigFile<Config> {
    selector: SysctlInterfaceSelector,
    _marker: std::marker::PhantomData<Config>,
}

impl<Config> BytesFileOps for InterfaceConfigFile<Config>
where
    Config: InterfaceConfig,
{
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let config = Config::try_from_i32(parse_i32_file(&data)?)?;
        set_interface_config(self.selector, &config)?;
        Ok(())
    }
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let config = Config::try_into_i32(get_interface_config(self.selector)?)?;
        Ok(serialize_for_file::<i32>(config).into())
    }
}

fn set_interface_config(
    selector: SysctlInterfaceSelector,
    config: &fidl_fuchsia_net_interfaces_admin::Configuration,
) -> Result<(), Errno> {
    match selector {
        SysctlInterfaceSelector::All => {
            log_warn!("setting config for all network interfaces is ignored");
            Ok(())
        }
        SysctlInterfaceSelector::Default => {
            let control =
                connect_to_protocol_sync::<fnet_settings::ControlMarker>().map_err(|err| {
                    log_error!(
                        "failed to connect to {}: {:?}",
                        fnet_settings::ControlMarker::PROTOCOL_NAME,
                        err
                    );
                    errno!(EIO)
                })?;
            control
                .update_interface_defaults(config, zx::MonotonicInstant::INFINITE)
                .map_err(|err| {
                    log_error!("failed to set network interface config: {:?}", err);
                    errno!(EIO)
                })?
                .map_err(map_update_error)?;
            Ok(())
        }
        SysctlInterfaceSelector::Id(id) => {
            let root =
                connect_to_protocol_sync::<fnet_root::InterfacesMarker>().map_err(|err| {
                    log_error!(
                        "failed to connect to {}: {:?}",
                        fnet_root::InterfacesMarker::PROTOCOL_NAME,
                        err
                    );
                    errno!(EIO)
                })?;
            let (control, server) = fidl::endpoints::create_sync_proxy();
            root.get_admin(id.get(), server).map_err(|err| {
                log_error!("failed to get network interface: {:?}", err);
                errno!(EIO)
            })?;
            let _prev = control
                .set_configuration(config, zx::MonotonicInstant::INFINITE)
                .map_err(|err| {
                    if err.is_closed() {
                        log_error!("network interface {} went away", id);
                        errno!(ENODEV)
                    } else {
                        log_error!("failed to set network interface config: {:?}", err);
                        errno!(EIO)
                    }
                })?
                .map_err(|err| {
                    use fnet_interfaces_admin::ControlSetConfigurationError;
                    match err {
                        ControlSetConfigurationError::Ipv4ForwardingUnsupported
                        | ControlSetConfigurationError::Ipv4MulticastForwardingUnsupported
                        | ControlSetConfigurationError::Ipv4IgmpVersionUnsupported
                        | ControlSetConfigurationError::Ipv6ForwardingUnsupported
                        | ControlSetConfigurationError::Ipv6MulticastForwardingUnsupported
                        | ControlSetConfigurationError::Ipv6MldVersionUnsupported
                        | ControlSetConfigurationError::ArpNotSupported
                        | ControlSetConfigurationError::NdpNotSupported => errno!(ENOTSUP),
                        ControlSetConfigurationError::IllegalZeroValue
                        | ControlSetConfigurationError::IllegalNegativeValue => errno!(EINVAL),
                        ControlSetConfigurationError::__SourceBreaking { unknown_ordinal } => {
                            log_error!("unknown error with ordinal: {unknown_ordinal}");
                            errno!(EIO)
                        }
                    }
                });
            Ok(())
        }
    }
}

fn get_interface_defaults() -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno> {
    let state = connect_to_protocol_sync::<fnet_settings::StateMarker>().map_err(|err| {
        log_error!("failed to connect to {}: {:?}", fnet_settings::StateMarker::PROTOCOL_NAME, err);
        errno!(EIO)
    })?;
    let config = state.get_interface_defaults(zx::MonotonicInstant::INFINITE).map_err(|err| {
        log_error!("failed to get network interface defaults: {:?}", err);
        if err.is_closed() { errno!(ENODEV) } else { errno!(EIO) }
    })?;
    Ok(config)
}

fn get_interface_config(
    selector: SysctlInterfaceSelector,
) -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno> {
    match selector {
        SysctlInterfaceSelector::All => {
            track_stub!(
                TODO("https://fxbug.dev/521344735"),
                "procfs sys net 'all' interface config"
            );
            get_interface_defaults()
        }
        SysctlInterfaceSelector::Default => get_interface_defaults(),
        SysctlInterfaceSelector::Id(id) => {
            let root =
                connect_to_protocol_sync::<fnet_root::InterfacesMarker>().map_err(|err| {
                    log_error!(
                        "failed to connect to {}: {:?}",
                        fnet_root::InterfacesMarker::PROTOCOL_NAME,
                        err
                    );
                    errno!(EIO)
                })?;
            let (control, server) = fidl::endpoints::create_sync_proxy();
            root.get_admin(id.get(), server).map_err(|err| {
                log_error!("failed to get network interface: {:?}", err);
                errno!(EIO)
            })?;
            let config = control
                .get_configuration(zx::MonotonicInstant::INFINITE)
                .map_err(|err| {
                    log_error!("failed to get network interface config: {:?}", err);
                    if err.is_closed() { errno!(ENODEV) } else { errno!(EIO) }
                })?
                .map_err(|err| match err {
                    fnet_interfaces_admin::ControlGetConfigurationError::__SourceBreaking {
                        unknown_ordinal,
                    } => {
                        log_error!("unknown error with ordinal: {unknown_ordinal}");
                        errno!(EIO)
                    }
                })?;
            Ok(config)
        }
    }
}

struct DisableIpv6;

impl InterfaceConfig for DisableIpv6 {
    fn try_from_i32(value: i32) -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno> {
        Ok(fidl_fuchsia_net_interfaces_admin::Configuration {
            ipv6: Some(fidl_fuchsia_net_interfaces_admin::Ipv6Configuration {
                enabled: Some(value == 0),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
    fn try_into_i32(
        config: fidl_fuchsia_net_interfaces_admin::Configuration,
    ) -> Result<i32, Errno> {
        let ipv6 = config.ipv6.ok_or_else(|| {
            log_error!("network interface config missing ipv6");
            errno!(EIO)
        })?;
        let enabled = ipv6.enabled.ok_or_else(|| {
            log_error!("network interface config missing ipv6 enabled");
            errno!(EIO)
        })?;
        Ok(i32::from(!enabled))
    }
}

struct UcastSolicit<I: Ip> {
    _marker: core::marker::PhantomData<I>,
}

impl<I: Ip> InterfaceConfig for UcastSolicit<I> {
    fn try_from_i32(value: i32) -> Result<fnet_interfaces_admin::Configuration, Errno> {
        let max_unicast_solicitations = u16::try_from(value).map_err(|_| errno!(EINVAL))?;
        let nud_config = fnet_interfaces_admin::NudConfiguration {
            max_unicast_solicitations: Some(max_unicast_solicitations),
            ..Default::default()
        };
        let mut config = fnet_interfaces_admin::Configuration::default();
        match I::VERSION {
            IpVersion::V4 => {
                config.ipv4 = Some(fidl_fuchsia_net_interfaces_admin::Ipv4Configuration {
                    arp: Some(fnet_interfaces_admin::ArpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
            IpVersion::V6 => {
                config.ipv6 = Some(fidl_fuchsia_net_interfaces_admin::Ipv6Configuration {
                    ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
        }
        Ok(config)
    }

    fn try_into_i32(config: fnet_interfaces_admin::Configuration) -> Result<i32, Errno> {
        let max_unicast_solicitations = match I::VERSION {
            IpVersion::V4 => config
                .ipv4
                .and_then(|ipv4| ipv4.arp)
                .and_then(|arp| arp.nud)
                .and_then(|nud| nud.max_unicast_solicitations)
                .ok_or_else(|| {
                    log_error!(
                        "network interface config missing ipv4 arp max_unicast_solicitations"
                    );
                    errno!(EIO)
                })?,
            IpVersion::V6 => config
                .ipv6
                .and_then(|ipv6| ipv6.ndp)
                .and_then(|ndp| ndp.nud)
                .and_then(|nud| nud.max_unicast_solicitations)
                .ok_or_else(|| {
                    log_error!(
                        "network interface config missing ipv6 ndp max_unicast_solicitations"
                    );
                    errno!(EIO)
                })?,
        };
        Ok(i32::from(max_unicast_solicitations))
    }
}

struct McastResolicit<I: Ip> {
    _marker: core::marker::PhantomData<I>,
}

impl<I: Ip> InterfaceConfig for McastResolicit<I> {
    fn try_from_i32(value: i32) -> Result<fnet_interfaces_admin::Configuration, Errno> {
        let max_multicast_solicitations = u16::try_from(value).map_err(|_| errno!(EINVAL))?;
        let nud_config = fnet_interfaces_admin::NudConfiguration {
            max_multicast_solicitations: Some(max_multicast_solicitations),
            ..Default::default()
        };
        let mut config = fnet_interfaces_admin::Configuration::default();
        match I::VERSION {
            IpVersion::V4 => {
                config.ipv4 = Some(fidl_fuchsia_net_interfaces_admin::Ipv4Configuration {
                    arp: Some(fnet_interfaces_admin::ArpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
            IpVersion::V6 => {
                config.ipv6 = Some(fidl_fuchsia_net_interfaces_admin::Ipv6Configuration {
                    ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
        }
        Ok(config)
    }

    fn try_into_i32(config: fnet_interfaces_admin::Configuration) -> Result<i32, Errno> {
        let max_multicast_solicitations = match I::VERSION {
            IpVersion::V4 => config
                .ipv4
                .and_then(|ipv4| ipv4.arp)
                .and_then(|arp| arp.nud)
                .and_then(|nud| nud.max_multicast_solicitations)
                .ok_or_else(|| {
                    log_error!(
                        "network interface config missing ipv4 arp max_multicast_solicitations"
                    );
                    errno!(EIO)
                })?,
            IpVersion::V6 => config
                .ipv6
                .and_then(|ipv6| ipv6.ndp)
                .and_then(|ndp| ndp.nud)
                .and_then(|nud| nud.max_multicast_solicitations)
                .ok_or_else(|| {
                    log_error!(
                        "network interface config missing ipv6 ndp max_multicast_solicitations"
                    );
                    errno!(EIO)
                })?,
        };
        Ok(i32::from(max_multicast_solicitations))
    }
}

struct BaseReachableTimeMs<I: Ip> {
    _marker: core::marker::PhantomData<I>,
}

impl<I: Ip> InterfaceConfig for BaseReachableTimeMs<I> {
    fn try_from_i32(value: i32) -> Result<fnet_interfaces_admin::Configuration, Errno> {
        let base_reachable_time = zx::Duration::<zx::BootTimeline>::from_millis(i64::from(value));
        let nud_config = fnet_interfaces_admin::NudConfiguration {
            base_reachable_time: Some(base_reachable_time.into_nanos()),
            ..Default::default()
        };
        let mut config = fnet_interfaces_admin::Configuration::default();
        match I::VERSION {
            IpVersion::V4 => {
                config.ipv4 = Some(fnet_interfaces_admin::Ipv4Configuration {
                    arp: Some(fnet_interfaces_admin::ArpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
            IpVersion::V6 => {
                config.ipv6 = Some(fnet_interfaces_admin::Ipv6Configuration {
                    ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
        }
        Ok(config)
    }

    fn try_into_i32(config: fnet_interfaces_admin::Configuration) -> Result<i32, Errno> {
        let base_reachable_time_ns = match I::VERSION {
            IpVersion::V4 => config
                .ipv4
                .and_then(|ipv4| ipv4.arp)
                .and_then(|arp| arp.nud)
                .and_then(|nud| nud.base_reachable_time)
                .ok_or_else(|| {
                    log_error!("network interface config missing ipv4 arp base_reachable_time");
                    errno!(EIO)
                })?,
            IpVersion::V6 => config
                .ipv6
                .and_then(|ipv6| ipv6.ndp)
                .and_then(|ndp| ndp.nud)
                .and_then(|nud| nud.base_reachable_time)
                .ok_or_else(|| {
                    log_error!("network interface config missing ipv6 ndp base_reachable_time");
                    errno!(EIO)
                })?,
        };
        Ok(i32::try_from(
            zx::Duration::<zx::BootTimeline>::from_nanos(base_reachable_time_ns).into_millis(),
        )
        .map_err(|_| errno!(EIO))?)
    }
}

struct Ipv6DadTransmits;

impl InterfaceConfig for Ipv6DadTransmits {
    fn try_from_i32(value: i32) -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno> {
        let transmits = u16::try_from(value).map_err(|_| errno!(EINVAL))?;
        Ok(fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    dad: Some(fnet_interfaces_admin::DadConfiguration {
                        transmits: Some(transmits),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    fn try_into_i32(
        config: fidl_fuchsia_net_interfaces_admin::Configuration,
    ) -> Result<i32, Errno> {
        config
            .ipv6
            .and_then(|ipv6| ipv6.ndp)
            .and_then(|ndp| ndp.dad)
            .and_then(|dad| dad.transmits)
            .map(i32::from)
            .ok_or_else(|| {
                log_error!("network interface config missing ipv6 ndp dad transmits");
                errno!(EIO)
            })
    }
}

// Note that this has a different behavior than Linux, linux does not tell
// whether a neighbor host variable is set by user or is learned from network.
// The Fuchsia behavior is the same if the value is only set once during
// initialization.
struct RetransTimeMs<I: Ip> {
    _marker: core::marker::PhantomData<I>,
}

impl<I: Ip> InterfaceConfig for RetransTimeMs<I> {
    fn try_from_i32(value: i32) -> Result<fnet_interfaces_admin::Configuration, Errno> {
        let retrans_timer = zx::Duration::<zx::BootTimeline>::from_millis(i64::from(value));
        let nud_config = fnet_interfaces_admin::NudConfiguration {
            retrans_timer: Some(retrans_timer.into_nanos()),
            ..Default::default()
        };
        let mut config = fnet_interfaces_admin::Configuration::default();
        match I::VERSION {
            IpVersion::V4 => {
                config.ipv4 = Some(fnet_interfaces_admin::Ipv4Configuration {
                    arp: Some(fnet_interfaces_admin::ArpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
            IpVersion::V6 => {
                config.ipv6 = Some(fnet_interfaces_admin::Ipv6Configuration {
                    ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                        nud: Some(nud_config),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            }
        }
        Ok(config)
    }

    fn try_into_i32(config: fnet_interfaces_admin::Configuration) -> Result<i32, Errno> {
        let retrans_timer_ns = match I::VERSION {
            IpVersion::V4 => config
                .ipv4
                .and_then(|ipv4| ipv4.arp)
                .and_then(|arp| arp.nud)
                .and_then(|nud| nud.retrans_timer)
                .ok_or_else(|| {
                    log_error!("network interface config missing ipv4 arp retrans_timer");
                    errno!(EIO)
                })?,
            IpVersion::V6 => config
                .ipv6
                .and_then(|ipv6| ipv6.ndp)
                .and_then(|ndp| ndp.nud)
                .and_then(|nud| nud.retrans_timer)
                .ok_or_else(|| {
                    log_error!("network interface config missing ipv6 ndp retrans_timer");
                    errno!(EIO)
                })?,
        };
        Ok(i32::try_from(
            zx::Duration::<zx::BootTimeline>::from_nanos(retrans_timer_ns).into_millis(),
        )
        .map_err(|_| errno!(EIO))?)
    }
}

struct UseTempAddr;

impl InterfaceConfig for UseTempAddr {
    fn try_from_i32(value: i32) -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno> {
        // use_tempaddr - INTEGER
        // Preference for Privacy Extensions (RFC3041).
        // <= 0 : disable Privacy Extensions
        // == 1 : enable Privacy Extensions, but prefer public
        //      addresses over temporary addresses.
        // >  1 : enable Privacy Extensions and prefer temporary
        //      addresses over public addresses.
        //
        // Netstack only supports disable (<=0) or enable (>1). 1 is not a
        // sensible option. We will make it more strict by interpreting 1 as
        // >1.
        if value == 1 {
            log_warn!(
                "use_tempaddr=1 is not supported, treating it as enabled and we will prefer temporary addresses over public addresses"
            );
        }
        let use_tempaddr = value >= 1;
        Ok(fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    slaac: Some(fnet_interfaces_admin::SlaacConfiguration {
                        temporary_address: Some(use_tempaddr),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    fn try_into_i32(
        config: fidl_fuchsia_net_interfaces_admin::Configuration,
    ) -> Result<i32, Errno> {
        config
            .ipv6
            .and_then(|ipv6| ipv6.ndp)
            .and_then(|ndp| ndp.slaac)
            .and_then(|slacc| slacc.temporary_address)
            // We deviate from Linux here by not remembering the original
            // value, this is acceptable for now and we should revisit if it
            // causes issues.
            .map(|use_tempaddr| if use_tempaddr { 2 } else { 0 })
            .ok_or_else(|| {
                log_error!("network interface config missing ipv6 ndp slacc temporary_address");
                errno!(EIO)
            })
    }
}

struct AcceptRaDefrtr;

impl InterfaceConfig for AcceptRaDefrtr {
    fn try_from_i32(value: i32) -> Result<fidl_fuchsia_net_interfaces_admin::Configuration, Errno> {
        Ok(fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    route_discovery: Some(fnet_interfaces_admin::RouteDiscoveryConfiguration {
                        allow_default_route: Some(value != 0),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    fn try_into_i32(
        config: fidl_fuchsia_net_interfaces_admin::Configuration,
    ) -> Result<i32, Errno> {
        config
            .ipv6
            .and_then(|ipv6| ipv6.ndp)
            .and_then(|ndp| ndp.route_discovery)
            .and_then(|route_discovery| route_discovery.allow_default_route)
            .map(|allow_default_route| i32::from(allow_default_route))
            .ok_or_else(|| {
                log_error!(
                    "network interface config missing ipv6 ndp route_discovery allow_default_route"
                );
                errno!(EIO)
            })
    }
}

pub struct TcpRmemFile;

impl TcpRmemFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self)
    }
}

impl BytesFileOps for TcpRmemFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let mut params = std::str::from_utf8(&data)
            .map_err(|_| errno!(EINVAL))?
            .trim_ascii()
            .split_ascii_whitespace();

        let min = params
            .next()
            .ok_or_else(|| errno!(EINVAL))?
            .parse::<u32>()
            .map_err(|_| errno!(EINVAL))?;
        let default = params
            .next()
            .ok_or_else(|| errno!(EINVAL))?
            .parse::<u32>()
            .map_err(|_| errno!(EINVAL))?;
        let max = params
            .next()
            .ok_or_else(|| errno!(EINVAL))?
            .parse::<u32>()
            .map_err(|_| errno!(EINVAL))?;

        if params.next().is_some() {
            return error!(EINVAL);
        }

        let control =
            connect_to_protocol_sync::<fnet_settings::ControlMarker>().map_err(|err| {
                log_error!(
                    "failed to connect to {}: {:?}",
                    fnet_settings::ControlMarker::PROTOCOL_NAME,
                    err
                );
                errno!(EIO)
            })?;

        let tcp_settings = fnet_settings::Tcp {
            buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                receive: Some(fnet_settings::SocketBufferSizeRange {
                    min: Some(min),
                    default: Some(default),
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_tcp(&tcp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update tcp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        Ok(())
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let state = connect_to_protocol_sync::<fnet_settings::StateMarker>().map_err(|err| {
            log_error!(
                "failed to connect to {}: {:?}",
                fnet_settings::StateMarker::PROTOCOL_NAME,
                err
            );
            errno!(EIO)
        })?;

        let tcp_settings = state.get_tcp(zx::MonotonicInstant::INFINITE).map_err(|err| {
            log_error!("failed to get tcp settings: {:?}", err);
            errno!(EIO)
        })?;

        let receive_sizes =
            tcp_settings.buffer_sizes.and_then(|sizes| sizes.receive).ok_or_else(|| {
                log_error!("tcp settings missing receive buffer sizes");
                errno!(EIO)
            })?;

        let min = receive_sizes.min.unwrap_or(0);
        let default = receive_sizes.default.unwrap_or(0);
        let max = receive_sizes.max.unwrap_or(0);

        Ok(format!("{}\t{}\t{}\n", min, default, max).into_bytes().into())
    }
}

pub struct RmemMaxFile;

impl RmemMaxFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self)
    }
}

fn map_update_error(err: fnet_settings::UpdateError) -> Errno {
    match err {
        fnet_settings::UpdateError::IllegalZeroValue
        | fnet_settings::UpdateError::IllegalNegativeValue => errno!(EINVAL),
        fnet_settings::UpdateError::OutOfRange => errno!(ERANGE),
        fnet_settings::UpdateError::NotSupported => errno!(ENOTSUP),
        fnet_settings::UpdateError::__SourceBreaking { unknown_ordinal } => {
            log_error!("unknown error with ordinal: {unknown_ordinal}");
            errno!(EIO)
        }
    }
}

impl BytesFileOps for RmemMaxFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let max = std::str::from_utf8(&data)
            .map_err(|_| errno!(EINVAL))?
            .trim_ascii()
            .parse::<u32>()
            .map_err(|_| errno!(EINVAL))?;

        let control =
            connect_to_protocol_sync::<fnet_settings::ControlMarker>().map_err(|err| {
                log_error!(
                    "failed to connect to {}: {:?}",
                    fnet_settings::ControlMarker::PROTOCOL_NAME,
                    err
                );
                errno!(EIO)
            })?;

        let tcp_settings = fnet_settings::Tcp {
            buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                receive: Some(fnet_settings::SocketBufferSizeRange {
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_tcp(&tcp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update tcp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        let udp_settings = fnet_settings::Udp {
            buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                receive: Some(fnet_settings::SocketBufferSizeRange {
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_udp(&udp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update udp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        let icmp_settings = fnet_settings::Icmp {
            echo_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                receive: Some(fnet_settings::SocketBufferSizeRange {
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_icmp(&icmp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update icmp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        Ok(())
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let state = connect_to_protocol_sync::<fnet_settings::StateMarker>().map_err(|err| {
            log_error!(
                "failed to connect to {}: {:?}",
                fnet_settings::StateMarker::PROTOCOL_NAME,
                err
            );
            errno!(EIO)
        })?;

        // Note: The three max values may have changed and not agree with each other.
        // Currently this is good enough to only get one of the max values.
        let tcp_settings = state.get_tcp(zx::MonotonicInstant::INFINITE).map_err(|err| {
            log_error!("failed to get tcp settings: {:?}", err);
            errno!(EIO)
        })?;

        let max = tcp_settings
            .buffer_sizes
            .and_then(|sizes| sizes.receive)
            .and_then(|sizes| sizes.max)
            .ok_or_else(|| {
                log_error!("tcp settings missing receive buffer sizes");
                errno!(EIO)
            })?;

        Ok(format!("{}\n", max).into_bytes().into())
    }
}

pub struct WmemMaxFile;

impl WmemMaxFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self)
    }
}

impl BytesFileOps for WmemMaxFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let max = std::str::from_utf8(&data)
            .map_err(|_| errno!(EINVAL))?
            .trim_ascii()
            .parse::<u32>()
            .map_err(|_| errno!(EINVAL))?;

        let control =
            connect_to_protocol_sync::<fnet_settings::ControlMarker>().map_err(|err| {
                log_error!(
                    "failed to connect to {}: {:?}",
                    fnet_settings::ControlMarker::PROTOCOL_NAME,
                    err
                );
                errno!(EIO)
            })?;

        let tcp_settings = fnet_settings::Tcp {
            buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                send: Some(fnet_settings::SocketBufferSizeRange {
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_tcp(&tcp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update tcp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        let udp_settings = fnet_settings::Udp {
            buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                send: Some(fnet_settings::SocketBufferSizeRange {
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_udp(&udp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update udp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        let icmp_settings = fnet_settings::Icmp {
            echo_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                send: Some(fnet_settings::SocketBufferSizeRange {
                    max: Some(max),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        control
            .update_icmp(&icmp_settings, zx::MonotonicInstant::INFINITE)
            .map_err(|err| {
                log_error!("failed to update icmp settings: {:?}", err);
                errno!(EIO)
            })?
            .map_err(map_update_error)?;

        Ok(())
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let state = connect_to_protocol_sync::<fnet_settings::StateMarker>().map_err(|err| {
            log_error!(
                "failed to connect to {}: {:?}",
                fnet_settings::StateMarker::PROTOCOL_NAME,
                err
            );
            errno!(EIO)
        })?;

        // Note: The three max values may have changed and not agree with each other.
        // Currently this is good enough to only get one of the max values.
        let tcp_settings = state.get_tcp(zx::MonotonicInstant::INFINITE).map_err(|err| {
            log_error!("failed to get tcp settings: {:?}", err);
            errno!(EIO)
        })?;

        let max = tcp_settings
            .buffer_sizes
            .and_then(|sizes| sizes.send)
            .and_then(|sizes| sizes.max)
            .ok_or_else(|| {
                log_error!("tcp settings missing send buffer sizes");
                errno!(EIO)
            })?;

        Ok(format!("{}\n", max).into_bytes().into())
    }
}
