// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::Device;
use anyhow::Error;
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServerEnd};
use fidl_fuchsia_driver_framework::{BusInfo, BusType, DeviceAddress};
use fidl_fuchsia_driver_token as ftoken;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockMarker;
use fs_management::filesystem::{BlockConnector, DirBasedBlockConnector};
use itertools::Itertools as _;
use pseudo_fs::{LazyPseudoDirectoryAsync, PseudoDirectory, PseudoFile, ToPseudoDirectoryAsync};
use std::sync::Arc;
use vfs::ExecutionScope;
use vfs::directory::helper::DirectlyMutable;
use vfs::service::endpoint;

pub trait SinglePublisher: Send + Sync {
    fn publish(self: Box<Self>, device: &dyn Device) -> Result<(), Error>;
}

/// A staged entry is one where the entry is placed in the block directory but it hasn't been
/// matched yet. Once it's matched, the matcher can use this to finish publishing the device. Until
/// it is completely published, open requests will be queued in the standard dangling-server-end
/// pipelining way.
pub struct StagedPublisher {
    scope: ExecutionScope,
    server_end: ServerEnd<fio::DirectoryMarker>,
}

impl StagedPublisher {
    fn new(scope: ExecutionScope) -> (fio::DirectoryProxy, Self) {
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        (proxy, Self { scope, server_end })
    }
}

impl SinglePublisher for StagedPublisher {
    fn publish(self: Box<Self>, device: &dyn Device) -> Result<(), Error> {
        let volume = device.block_connector()?;
        let entry = vfs::pseudo_directory! {
            BlockMarker::PROTOCOL_NAME => endpoint(move |_scope, channel| {
                volume.connect_channel_to_block(channel.into_zx_channel().into())
                    .unwrap_or_else(|error| {
                        log::error!(error:%; "failed to open volume");
                    });
            }),
        };
        vfs::directory::serve_on(entry, fio::PERM_READABLE, self.scope, self.server_end);

        Ok(())
    }
}

pub struct DevicePublisher {
    scope: ExecutionScope,

    // The inner block directories are LazyPseudoDirectory directories and not this directory
    // because this directory is exposed from fshost and component_manager calls
    // fuchsia.unknown/Cloneable.Clone on it. The Clone call would force LazyPseudoDirectory to
    // materialize the directory eliminating any of the potential savings.
    debug_block_dir: Arc<PseudoDirectory>,

    block_dir: Arc<PseudoDirectory>,
}

impl DevicePublisher {
    pub fn new(scope: ExecutionScope) -> Self {
        Self { scope, debug_block_dir: PseudoDirectory::new(), block_dir: PseudoDirectory::new() }
    }

    /// Publishes *all* block devices.  Only suitable for routing outside of fshost on eng
    /// configurations.
    pub fn debug_block_dir(&self) -> Arc<PseudoDirectory> {
        self.debug_block_dir.clone()
    }

    /// Publishes block devices which are not already managed by fshost (i.e. block devices from the
    /// low-level storage drivers).
    pub fn block_dir(&self) -> Arc<PseudoDirectory> {
        self.block_dir.clone()
    }

    pub fn publish_to_debug_block_dir(&self, device: &dyn Device, name: &str) -> Result<(), Error> {
        let name = name.to_string();
        let debug_block_dir = self.debug_block_dir.clone();
        let source = device.source().to_string();

        let connector = match device.service_instance_directory() {
            Ok(service_dir) => BlockConnectorType::Service(DirBasedBlockConnector::new(
                service_dir,
                "volume".to_string(),
            )),
            Err(_) => {
                let volume = device.block_connector()?;
                BlockConnectorType::Connector(volume)
            }
        };

        let info = BlockDirectoryInfo { connector, source: source.into_bytes().into_boxed_slice() };
        debug_block_dir.add_entry(&name, LazyPseudoDirectoryAsync::new(info))?;
        Ok(())
    }

    pub fn stage(&self, name: &str) -> Result<StagedPublisher, Error> {
        let (proxy, staged_publisher) = StagedPublisher::new(self.scope.clone());
        self.block_dir.add_entry(name, vfs::remote::remote_dir(proxy))?;
        Ok(staged_publisher)
    }

    pub fn publish(&mut self, volume: Box<dyn BlockConnector>, name: &str) -> Result<(), Error> {
        self.block_dir.add_entry(
            name,
            vfs::pseudo_directory! {
                BlockMarker::PROTOCOL_NAME => endpoint(move |_scope, channel| {
                    volume.connect_channel_to_block(channel.into_zx_channel().into())
                        .unwrap_or_else(|error| {
                            log::error!(error:%; "failed to open volume");
                        });
                }),
            },
        )?;
        Ok(())
    }
}

// Helper for Display
// TODO(https://fxbug.dev/512559396): This should be in a library.
struct BusPathType<'a>(&'a Option<BusType>);

impl<'a> std::fmt::Display for BusPathType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(BusType::Platform) => write!(f, "platform"),
            Some(BusType::Acpi) => write!(f, "acpi"),
            Some(BusType::DeviceTree) => write!(f, "device-tree"),
            Some(BusType::Pci) => write!(f, "pci"),
            Some(BusType::Usb) => write!(f, "usb"),
            Some(BusType::Gpio) => write!(f, "gpio"),
            Some(BusType::I2C) => write!(f, "i2c"),
            Some(BusType::Spi) => write!(f, "spi"),
            Some(BusType::Sdio) => write!(f, "sdio"),
            Some(BusType::Uart) => write!(f, "uart"),
            Some(BusType::Spmi) => write!(f, "spmi"),
            Some(BusType::UsbPeripheral) => write!(f, "usb-peripheral"),
            Some(BusType::Virtio) => write!(f, "virtio"),
            Some(_) => write!(f, "<unknown>"),
            None => write!(f, "<none>"),
        }
    }
}

// Helper for Display
// TODO(https://fxbug.dev/512559396): This should be in a library.
struct BusPathAddress<'a>(&'a Option<DeviceAddress>);

impl<'a> std::fmt::Display for BusPathAddress<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(DeviceAddress::IntValue(val)) => write!(f, "{val:02X}"),
            Some(DeviceAddress::ArrayIntValue(val)) => {
                write!(f, "{}", val.iter().map(|v| format!("{v:02X}")).join(":"))
            }
            Some(DeviceAddress::CharIntValue(val)) => write!(f, "{val}"),
            Some(DeviceAddress::ArrayCharIntValue(val)) => {
                write!(f, "{}", val.iter().map(|v| v.to_string()).join(":"))
            }
            Some(DeviceAddress::StringValue(val)) => write!(f, "{val}"),
            Some(_) => write!(f, "<unknown>"),
            None => write!(f, "<none>"),
        }
    }
}

// Helper for Display
// TODO(https://fxbug.dev/512559396): This should be in a library.
struct BusPathElement<'a>(&'a BusInfo);

impl<'a> std::fmt::Display for BusPathElement<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let (Some(BusType::Pci), Some(DeviceAddress::ArrayIntValue(val))) =
            (&self.0.bus, &self.0.address)
        {
            // PCI addresses are traditionally formatted using BDF notation (bus:device.function).
            // The array contains [bus, device, function].
            if val.len() == 3 {
                return write!(f, "pci{:02X}:{:02X}.{:X}", val[0], val[1], val[2]);
            }
        }
        write!(f, "{}{}", BusPathType(&self.0.bus), BusPathAddress(&self.0.address))
    }
}

// Helper for Display
// TODO(https://fxbug.dev/512559396): This should be in a library.
struct BusPath(Vec<BusInfo>);

impl std::fmt::Display for BusPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, info) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, "/")?;
            }
            write!(f, "{}", BusPathElement(info))?;
        }
        Ok(())
    }
}

// TODO(https://fxbug.dev/470140477): Remove this and directly use DirBasedBlockConnector once all
// drivers use services.
enum BlockConnectorType {
    Service(DirBasedBlockConnector),
    Connector(Box<dyn BlockConnector>),
}

impl BlockConnectorType {
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error> {
        match self {
            Self::Service(service) => service.connect_channel_to_block(server_end),
            Self::Connector(connector) => connector.connect_channel_to_block(server_end),
        }
    }
}

struct BlockDirectoryInfo {
    connector: BlockConnectorType,
    source: Box<[u8]>,
}

impl BlockDirectoryInfo {
    async fn get_bus_path(&self) -> Result<BusPath, Error> {
        let service_dir = match &self.connector {
            BlockConnectorType::Service(s) => s,
            BlockConnectorType::Connector(_) => {
                return Err(anyhow::anyhow!("Not a service instance"));
            }
        };
        let token_client = fuchsia_component::client::connect_to_named_protocol_at_dir_root::<
            ftoken::NodeTokenMarker,
        >(&service_dir.dir(), "token")?;
        let token = token_client.get().await?.map_err(zx::Status::err_from_raw)?;
        let bus_topo_client =
            fuchsia_component::client::connect_to_protocol::<ftoken::NodeBusTopologyMarker>()?;
        let path = bus_topo_client.get(token).await?.map_err(zx::Status::err_from_raw)?;
        Ok(BusPath(path))
    }
}

impl ToPseudoDirectoryAsync for BlockDirectoryInfo {
    fn to_pseudo_directory(self) -> impl std::future::Future<Output = Arc<PseudoDirectory>> + Send {
        async move {
            let bus_path =
                self.get_bus_path().await.map(|b| b.to_string()).unwrap_or_else(|error| {
                    log::warn!(error:%; "failed to get bus path");
                    "<unknown>".to_string()
                });
            vfs::pseudo_directory! {
                BlockMarker::PROTOCOL_NAME => endpoint(move |_scope, channel| {
                    self.connector.connect_channel_to_block(channel.into_zx_channel().into())
                        .unwrap_or_else(|error| {
                            log::error!(error:%; "failed to open volume");
                        });
                }),
                "source" => PseudoFile::from_data(self.source),
                "bus_path" => PseudoFile::from_data(bus_path.into_bytes()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Parent;
    use async_trait::async_trait;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_io::DirectoryProxy;
    use fidl_fuchsia_storage_block::BlockProxy;
    use fs_management::format::DiskFormat;
    use fuchsia_fs::directory::read_file_to_string;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use zx::AsHandleRef;

    struct MockDevice {
        source: &'static str,
        scope: ExecutionScope,
        dir: Arc<PseudoDirectory>,
    }

    #[async_trait]
    impl Device for MockDevice {
        async fn get_block_info(&self) -> Result<fidl_fuchsia_storage_block::BlockInfo, Error> {
            unimplemented!()
        }
        fn is_nand(&self) -> bool {
            unimplemented!()
        }
        async fn content_format(&mut self) -> Result<DiskFormat, Error> {
            unimplemented!()
        }
        fn topological_path(&self) -> &str {
            unimplemented!()
        }
        fn path(&self) -> &str {
            unimplemented!()
        }
        fn source(&self) -> &str {
            self.source
        }
        fn parent(&self) -> Parent {
            unimplemented!()
        }
        async fn partition_label(&mut self) -> Result<&str, Error> {
            unimplemented!()
        }
        async fn partition_type(&mut self) -> Result<&[u8; 16], Error> {
            unimplemented!()
        }
        fn service_instance_directory(&self) -> Result<DirectoryProxy, Error> {
            let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            vfs::directory::serve_on(
                self.dir.clone(),
                fio::PERM_READABLE,
                self.scope.clone(),
                server_end,
            );
            Ok(proxy)
        }
        fn block_connector(&self) -> Result<Box<dyn BlockConnector>, Error> {
            let dir = self.service_instance_directory()?;
            Ok(Box::new(DirBasedBlockConnector::new(dir, "volume".to_string())))
        }
        fn block_proxy(&self) -> Result<BlockProxy, Error> {
            unimplemented!()
        }
        fn is_fshost_ramdisk(&self) -> bool {
            unimplemented!()
        }
        fn set_fshost_ramdisk(&mut self, _v: bool) {
            unimplemented!()
        }
    }

    #[fuchsia::test]
    async fn test_publish_to_debug_block_dir() {
        let scope = ExecutionScope::new();
        let device_publisher = DevicePublisher::new(scope.clone());
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        vfs::directory::serve_on(
            device_publisher.debug_block_dir(),
            fio::PERM_READABLE,
            scope.clone(),
            server_end,
        );
        let (sender, mut receiver) = mpsc::unbounded();
        let dir = vfs::pseudo_directory! {
            "volume" => endpoint(move |_scope, channel| {
                sender.unbounded_send(channel.into_zx_channel().into()).unwrap();
            }),
        };
        let mock_device = MockDevice { source: "mock-source", scope: scope.clone(), dir };
        device_publisher.publish_to_debug_block_dir(&mock_device, "001").unwrap();

        // Check that the "source" file is correct.
        assert_eq!(
            read_file_to_string(&client, "001/source").await.expect("failed to read file"),
            "mock-source"
        );

        // Check that connecting to the volume works.
        let path = "001/".to_string() + BlockMarker::PROTOCOL_NAME;
        let volume = fuchsia_fs::directory::open_async::<BlockMarker>(
            &client,
            &path,
            fio::Flags::PROTOCOL_SERVICE,
        )
        .unwrap();
        let volume_server_end: fidl::endpoints::ServerEnd<BlockMarker> =
            receiver.next().await.unwrap();
        assert_eq!(
            volume.as_channel().as_handle_ref().basic_info().unwrap().related_koid,
            volume_server_end.channel().as_handle_ref().koid().unwrap()
        );
    }
}
