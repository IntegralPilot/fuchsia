// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file contains code for creating and serving tun/tap devices.

use fidl::endpoints::Proxy as _;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_tun as fnet_tun;
use fuchsia_async as fasync;
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::security;
use starnix_core::task::{CurrentTask, RunState, WaiterRef};
use starnix_core::vfs::socket::IfReqPtr;
use starnix_core::vfs::{FileObject, FileObjectState, FileOps, call_fidl_and_await_close};
use starnix_logging::{log_info, log_warn};
use starnix_sync::{LockDepMutex, TunDevTunLock};
use starnix_uapi::auth::CAP_NET_ADMIN;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use std::num::NonZeroU64;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
enum DevKind {
    Tun,
    Tap,
}

impl DevKind {
    fn rx_types(&self) -> impl IntoIterator<Item = fhardware_network::FrameType> {
        match self {
            DevKind::Tun => itertools::Either::Left(
                [fhardware_network::FrameType::Ipv4, fhardware_network::FrameType::Ipv6]
                    .into_iter(),
            ),
            DevKind::Tap => {
                itertools::Either::Right(std::iter::once(fhardware_network::FrameType::Ethernet))
            }
        }
    }

    fn tx_types(&self) -> impl IntoIterator<Item = fhardware_network::FrameTypeSupport> {
        self.rx_types().into_iter().map(|frame_type| fhardware_network::FrameTypeSupport {
            type_: frame_type,
            features: 0,
            supported_flags: fhardware_network::TxFlags::empty(),
        })
    }
}

fn random_mac() -> fnet::MacAddress {
    let mut octets = [0u8; 6];
    starnix_crypto::cprng_draw(&mut octets[..]);
    // Ensure the least-significant-bit of the first byte of the address is 0,
    // indicating that it is a unicast address.
    // https://en.wikipedia.org/wiki/MAC_address#Unicast_vs._multicast
    octets[0] = octets[0] & !1;

    // Ensure the second-least-significant bit of the first byte of the address
    // is 1, indicating the address is locally administered (i.e. assigned by
    // software and not by a device manufacturer).
    // https://en.wikipedia.org/wiki/MAC_address#Universal_vs._local_(U/L_bit)
    octets[0] = octets[0] | 0b10;

    fnet::MacAddress { octets }
}

#[derive(Debug)]
struct CreateTunRequest {
    name: String,
    kind: DevKind,
    // If true, will report frame metadata on receiving frames.
    report_metadata: bool,
}

// Give back `ClientEnd`s so that clients can use synchronous proxies for
// reading/writing operations on the files.
struct CreateTunResponse {
    device: fidl::endpoints::ClientEnd<fidl_fuchsia_net_tun::DeviceMarker>,
    port: fidl::endpoints::ClientEnd<fidl_fuchsia_net_tun::PortMarker>,
    port_info: fhardware_network::PortInfo,
    interface_id: NonZeroU64,
}

const ARBITRARY_PORT_ID: u8 = 2;
const ETHERNET_MTU: u32 = 1500;
const MAX_ETHERNET_HEADER_SIZE: u32 = 18;

macro_rules! errno_from_interfaces_admin_error {
    ($err:expr) => {
        match $err {
            fnet_interfaces_ext::admin::TerminalError::Terminal(err) => match err {
                fnet_interfaces_admin::InterfaceRemovedReason::DuplicateName => {
                    starnix_uapi::errno!(
                        EEXIST,
                        "tried to create tuntap interface with duplicate name"
                    )
                }
                fnet_interfaces_admin::InterfaceRemovedReason::PortAlreadyBound => {
                    starnix_uapi::errno!(EBUSY, "tuntap port already bound to an interface")
                }
                fnet_interfaces_admin::InterfaceRemovedReason::BadPort => {
                    starnix_uapi::errno!(ENOENT, "tried to create tuntap interface from bad port")
                }
                fnet_interfaces_admin::InterfaceRemovedReason::PortClosed => {
                    starnix_uapi::errno!(
                        ENOENT,
                        "tried to create tuntap interface from closed port"
                    )
                }
                fnet_interfaces_admin::InterfaceRemovedReason::User => {
                    starnix_uapi::errno!(
                        ENOENT,
                        "tuntap interface was removed out from under starnix"
                    )
                }
                fnet_interfaces_admin::InterfaceRemovedReasonUnknown!() => {
                    starnix_uapi::errno!(ENOENT, "unknown interface removed reason")
                }
            },
            fnet_interfaces_ext::admin::TerminalError::Fidl(e) => {
                starnix_uapi::errno!(ENOENT, format!("interfaces admin FIDL error: {e:?}"))
            }
        }
    };
}

struct TunWorker {
    tun_control: fidl_fuchsia_net_tun::ControlProxy,
    installer: fidl_fuchsia_net_interfaces_admin::InstallerProxy,
}

impl TunWorker {
    async fn handle_create_request(
        &mut self,
        request: CreateTunRequest,
    ) -> Result<CreateTunResponse, Errno> {
        let CreateTunRequest { name, kind, report_metadata } = request;
        if report_metadata {
            log_warn!(
                "TODO(https://fxbug.dev/332317144): frame-metadata reporting for
                 tuntap interfaces in starnix is not implemented yet"
            );
        }
        let report_metadata = false;

        let Self { tun_control, installer } = self;

        let (tun_device, server_end) =
            fidl::endpoints::create_endpoints::<fnet_tun::DeviceMarker>();
        tun_control
            .create_device(
                &fnet_tun::DeviceConfig {
                    blocking: Some(false),
                    base: Some(fnet_tun::BaseDeviceConfig {
                        report_metadata: Some(report_metadata),
                        min_tx_buffer_length: None,
                        min_rx_buffer_length: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                server_end,
            )
            .map_err(|e| {
                starnix_uapi::errno!(
                    ENOENT,
                    format!("creating fuchsia.net.tun Device failed: {e:?}")
                )
            })?;

        let (tun_port, server_end) = fidl::endpoints::create_endpoints::<fnet_tun::PortMarker>();

        let tun_device = tun_device.into_proxy();
        tun_device
            .add_port(
                &fnet_tun::DevicePortConfig {
                    base: Some(fnet_tun::BasePortConfig {
                        id: Some(ARBITRARY_PORT_ID),
                        // Even though this field is named `mtu`, it's actually
                        // the maximum frame size of whatever layer the
                        // interface operates at. So if we want to get to the
                        // typical 1500 MTU that we usually have above the
                        // Ethernet layer, for TAP devices we need to add space
                        // to account for the Ethernet header itself.
                        mtu: Some(
                            ETHERNET_MTU
                                + match kind {
                                    DevKind::Tun => 0,
                                    DevKind::Tap => MAX_ETHERNET_HEADER_SIZE,
                                },
                        ),
                        rx_types: Some(kind.rx_types().into_iter().collect::<Vec<_>>()),
                        tx_types: Some(kind.tx_types().into_iter().collect::<Vec<_>>()),
                        port_class: None,
                        ..Default::default()
                    }),
                    online: Some(true),
                    mac: match kind {
                        DevKind::Tun => None,
                        DevKind::Tap => Some(random_mac()),
                    },
                    ..Default::default()
                },
                server_end,
            )
            .map_err(|e| {
                starnix_uapi::errno!(ENOENT, format!("adding fuchsia.net.tun Port failed, {e:?}"))
            })?;

        let tun_port = tun_port.into_proxy();
        let (hw_port, server_end) =
            fidl::endpoints::create_endpoints::<fhardware_network::PortMarker>();
        tun_port.get_port(server_end).map_err(|e| {
            starnix_uapi::errno!(
                ENOENT,
                format!("getting fuchsia.hardware.networkn Port failed: {e:?}")
            )
        })?;
        let hw_port = hw_port.into_proxy();
        let port_info = hw_port.get_info().await.map_err(|e| {
            starnix_uapi::errno!(
                ENOENT,
                format!("getting fuchsia.hardware.network PortInfo failed: {e:?}")
            )
        })?;

        let (hw_device, server_end) =
            fidl::endpoints::create_endpoints::<fhardware_network::DeviceMarker>();
        tun_device.get_device(server_end).map_err(|e| {
            starnix_uapi::errno!(
                ENOENT,
                format!("getting fuchsia.hardware.network Device failed: {e:?}")
            )
        })?;

        let (device_control, server_end) =
            fidl::endpoints::create_endpoints::<fnet_interfaces_admin::DeviceControlMarker>();
        installer.install_device(hw_device, server_end).map_err(|e| {
            starnix_uapi::errno!(
                ENOENT,
                format!("installing fuchsia.hardware.network Device failed: {e:?}")
            )
        })?;

        let (interface_control, server_end) =
            fidl::endpoints::create_endpoints::<fnet_interfaces_admin::ControlMarker>();
        let device_control = device_control.into_proxy();
        device_control
            .create_interface(
                &port_info
                    .id
                    .ok_or_else(|| starnix_uapi::errno!(ENOENT, "got PortInfo with no ID"))?,
                server_end,
                fnet_interfaces_admin::Options {
                    name: Some(name.clone()),
                    metric: None,
                    ..Default::default()
                },
            )
            .map_err(|e| {
                starnix_uapi::errno!(
                    ENOENT,
                    format!("creating fuchsia.net.interfaces.admin Control failed: {e:?}")
                )
            })?;

        let interface_control = interface_control.into_proxy();
        let interface_control = fnet_interfaces_ext::admin::Control::new(interface_control);

        // Get the NICID that the netstack allocates for this interface. This
        // serves as a way to wait for the interface to be successfully
        // installed, verifying that there's no duplicate-name clash.
        let interface_id = interface_control
            .get_id()
            .await
            .map_err(|err| errno_from_interfaces_admin_error!(err))?;
        let interface_id = NonZeroU64::new(interface_id).expect("interface IDs must be nonzero");
        let _enabled: bool = interface_control
            .enable()
            .await
            .map_err(|err| errno_from_interfaces_admin_error!(err))?
            .map_err(|err: fnet_interfaces_admin::ControlEnableError| {
                starnix_uapi::errno!(
                    ENOENT,
                    format!("enabling fuchsia.net.interfaces.admin Control failed: {err:?}")
                )
            })?;

        let tun_device =
            tun_device.into_client_end().expect("should not have cloned tun_device proxy");
        let tun_port = tun_port.into_client_end().expect("should not have cloned tun_port proxy");

        // Detach the fnet_interfaces_admin DeviceControl to avoid it being
        // uninstalled once we drop the proxy. NB: we don't want to do this
        // until we're done doing any async work so that we clean up properly if
        // we're interrupted.
        device_control.detach().map_err(|e| {
            starnix_uapi::errno!(
                ENOENT,
                format!("detaching fuchsia.net.interfaces.admin DeviceControl failed: {e:?}")
            )
        })?;

        // Same for the fnet_interfaces_admin Control.
        interface_control.detach().map_err(|e| {
            starnix_uapi::errno!(
                ENOENT,
                format!("detaching fuchsia.net.interfaces.admin Control failed: {e:?}")
            )
        })?;

        Ok(CreateTunResponse { device: tun_device, port: tun_port, port_info, interface_id })
    }
}

#[derive(Default)]
pub struct DevTun(LockDepMutex<Option<DevTunInner>, TunDevTunLock>);

struct DevTunInner {
    _tun_device: fnet_tun::DeviceSynchronousProxy,
    tun_port: fnet_tun::PortSynchronousProxy,
    _port_info: fhardware_network::PortInfo,
    _interface_id: NonZeroU64,
}

impl FileOps for DevTun {
    starnix_core::fileops_impl_nonseekable!();
    starnix_core::fileops_impl_noop_sync!();

    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        // Ensure that these TUN resources are available for reuse by explicitly `remove()`ing the
        // port and waiting for that to be acknowledged, via the channel closing.
        let Some(state) = self.0.lock().take() else {
            return;
        };
        call_fidl_and_await_close(fnet_tun::PortSynchronousProxy::remove, &state.tun_port);
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn starnix_core::vfs::InputBuffer,
    ) -> Result<usize, Errno> {
        // TODO(https://fxbug.dev/332317144): Implement writing to a TUN/TAP
        // device.
        starnix_uapi::error!(EOPNOTSUPP)
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn starnix_core::vfs::OutputBuffer,
    ) -> Result<usize, Errno> {
        // TODO(https://fxbug.dev/332317144): Implement reading from a TUN/TAP
        // device.
        starnix_uapi::error!(EOPNOTSUPP)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: starnix_syscalls::SyscallArg,
    ) -> Result<starnix_syscalls::SyscallResult, Errno> {
        match request {
            starnix_uapi::TUNSETIFF => {
                // Linux requires CAP_NET_ADMIN in the network namespace for TUNSETIFF;
                // SELinux tun_socket create (check_tun_dev_create_access) is additional.
                security::check_task_capable(current_task, CAP_NET_ADMIN)?;
                security::check_tun_dev_create_access(current_task)?;
                log_info!("handling TUNSETIFF for /dev/tun");
                let user_addr = IfReqPtr::new(current_task, arg);
                let in_ifreq = current_task.read_multi_arch_object(user_addr)?;

                let name = in_ifreq.name_as_str()?.to_string();

                let flags = in_ifreq.ifru_flags() as u32;

                let iff_tun = flags & starnix_uapi::IFF_TUN != 0;
                let iff_tap = flags & starnix_uapi::IFF_TAP != 0;
                let iff_no_pi = flags & starnix_uapi::IFF_NO_PI != 0;

                let kind = match (iff_tun, iff_tap) {
                    (true, false) => DevKind::Tun,
                    (false, true) => DevKind::Tap,
                    _ => return starnix_uapi::error!(EINVAL),
                };

                let request = CreateTunRequest { name, kind, report_metadata: !iff_no_pi };

                log_info!("adding /dev/tun interface {request:?}");

                let (abort_handle, abort_registration) = futures::stream::AbortHandle::new_pair();
                let abort_handle = Arc::new(abort_handle);

                let CreateTunResponse { device, port, port_info, interface_id } = current_task
                    .run_in_state(
                        RunState::Waiter(WaiterRef::from_abort_handle(&abort_handle)),
                        || {
                            let mut executor = fasync::LocalExecutor::default();
                            let tun_control = fuchsia_component::client::connect_to_protocol::<
                                fnet_tun::ControlMarker,
                            >()
                            .map_err(|_| starnix_uapi::errno!(ENOENT))?;
                            let installer = fuchsia_component::client::connect_to_protocol::<
                                fnet_interfaces_admin::InstallerMarker,
                            >()
                            .map_err(|_| starnix_uapi::errno!(ENOENT))?;
                            let mut worker = TunWorker { tun_control, installer };
                            executor
                                .run_singlethreaded(futures::stream::Abortable::new(
                                    worker.handle_create_request(request),
                                    abort_registration,
                                ))
                                .map_err(|futures::stream::Aborted| starnix_uapi::errno!(EINTR))?
                        },
                    )?;

                *self.0.lock() = Some(DevTunInner {
                    _tun_device: device.into_sync_proxy(),
                    tun_port: port.into_sync_proxy(),
                    _port_info: port_info,
                    _interface_id: interface_id,
                });

                Ok(starnix_syscalls::SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}
