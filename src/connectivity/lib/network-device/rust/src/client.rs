// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia netdevice client.

use std::convert::TryInto as _;

use async_utils::hanging_get::client::HangingGetStream;
use fidl_fuchsia_hardware_network as netdev;
use fidl_table_validation::ValidFidlTable;
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use std::pin::pin;

use crate::error::{Error, Result};
use crate::session::{Config, DerivableConfig, DeviceInfo, Port, Session, Task};

#[derive(Clone)]
/// A client that communicates with a network device to send and receive packets.
pub struct Client {
    device: netdev::DeviceProxy,
}

/// A sensible default port stream buffer size.
const DEFAULT_PORT_STREAM_BUFFER_SIZE: u32 = 64;

/// Creates a [`Stream`] of [`PortStatus`] from a [`netdev::PortProxy`].
///
/// If `buffer` is `None`, a sensible nonzero default buffer size will be used.
pub fn new_port_status_stream(
    port_proxy: &netdev::PortProxy,
    buffer: Option<u32>,
) -> Result<impl Stream<Item = Result<PortStatus>> + Unpin + use<>> {
    let (watcher_proxy, watcher_server) =
        fidl::endpoints::create_proxy::<netdev::StatusWatcherMarker>();
    let () = port_proxy
        .get_status_watcher(watcher_server, buffer.unwrap_or(DEFAULT_PORT_STREAM_BUFFER_SIZE))?;
    Ok(HangingGetStream::new(watcher_proxy, netdev::StatusWatcherProxy::watch_status)
        .err_into()
        .and_then(|status| futures::future::ready(status.try_into().map_err(Error::PortStatus))))
}

impl Client {
    /// Creates a new network device client for the [`netdev::DeviceProxy`].
    pub fn new(device: netdev::DeviceProxy) -> Self {
        Client { device }
    }

    /// Connects to the specified `port`.
    pub fn connect_port(&self, port: Port) -> Result<netdev::PortProxy> {
        let (port_proxy, port_server) = fidl::endpoints::create_proxy::<netdev::PortMarker>();
        self.connect_port_server_end(port, port_server).map(move |()| port_proxy)
    }

    /// Connects to the specified `port` with the provided `server_end`.
    pub fn connect_port_server_end(
        &self,
        port: Port,
        server_end: fidl::endpoints::ServerEnd<netdev::PortMarker>,
    ) -> Result<()> {
        Ok(self.device.get_port(&port.into(), server_end)?)
    }

    /// Retrieves information about the underlying network device.
    pub async fn device_info(&self) -> Result<DeviceInfo> {
        Ok(self.device.get_info().await?.try_into()?)
    }

    /// Gets a [`Stream`] of [`PortStatus`] for status changes from the device.
    ///
    /// A sensible nonzero default buffer size will be used. It is encouraged
    /// to use this function.
    pub fn port_status_stream(
        &self,
        port: Port,
    ) -> Result<impl Stream<Item = Result<PortStatus>> + Unpin> {
        self.port_status_stream_with_buffer_size(port, DEFAULT_PORT_STREAM_BUFFER_SIZE)
    }

    /// Gets a [`Stream`] of [`PortStatus`] for status changes from the device.
    pub fn port_status_stream_with_buffer_size(
        &self,
        port: Port,
        buffer: u32,
    ) -> Result<impl Stream<Item = Result<PortStatus>> + Unpin> {
        let port_proxy = self.connect_port(port)?;
        new_port_status_stream(&port_proxy, Some(buffer))
    }

    /// Waits for `port` to become online and report the [`PortStatus`].
    pub async fn wait_online(&self, port: Port) -> Result<PortStatus> {
        let stream = self
            .port_status_stream_with_buffer_size(
                port, 0, /* we are not interested in every event */
            )?
            .try_filter(|status| {
                futures::future::ready(status.flags.contains(netdev::StatusFlags::ONLINE))
            });
        let mut stream = pin!(stream);
        stream.next().await.expect("HangingGetStream should never terminate")
    }

    /// Gets a [`Stream`] of [`DevicePortEvent`] to monitor port changes from the device.
    pub fn device_port_event_stream(
        &self,
    ) -> Result<impl Stream<Item = Result<DevicePortEvent>> + Unpin + use<>> {
        let (proxy, server) = fidl::endpoints::create_proxy::<netdev::PortWatcherMarker>();
        let () = self.device.get_port_watcher(server)?;
        Ok(HangingGetStream::new(proxy, netdev::PortWatcherProxy::watch).err_into())
    }

    /// Creates a new session with the given the given `name` and `config`.
    pub async fn new_session(&self, name: &str, config: Config) -> Result<(Session, Task)> {
        Session::new(&self.device, name, config).await
    }

    /// Creates a session using the higher level [`DerivableConfig`] instead of
    /// a raw [`Config`].
    pub async fn new_session_with_derivable_config(
        &self,
        name: &str,
        config: DerivableConfig,
    ) -> Result<(Session, Task)> {
        let device_info = self.device_info().await?;
        let config = device_info.make_config(config)?;
        Session::new(&self.device, name, config).await
    }
}

/// Network device base info with all required fields.
#[derive(Debug, Clone, ValidFidlTable)]
#[fidl_table_src(netdev::PortBaseInfo)]
#[fidl_table_strict]
pub struct PortBaseInfo {
    /// Port's class.
    pub port_class: netdev::PortClass,
    /// Supported rx frame types on this port.
    pub rx_types: Vec<netdev::FrameType>,
    /// Supported tx frame types on this port.
    pub tx_types: Vec<netdev::FrameTypeSupport>,
}

/// Network device information with all required fields.
#[derive(Debug, Clone, ValidFidlTable)]
#[fidl_table_src(netdev::PortInfo)]
#[fidl_table_strict]
pub struct PortInfo {
    /// Port's identifier.
    pub id: Port,
    /// Port's base info.
    pub base_info: PortBaseInfo,
}

/// Dynamic port information with all required fields.
#[derive(Debug, Clone, PartialEq, Eq, ValidFidlTable)]
#[fidl_table_src(netdev::PortStatus)]
#[fidl_table_strict]
pub struct PortStatus {
    /// Port status flags.
    pub flags: netdev::StatusFlags,
    /// Maximum transmit unit for this port, in bytes.
    ///
    /// The reported MTU is the size of an entire frame, including any header
    /// and trailer bytes for whatever protocols this port supports.
    pub mtu: u32,
}

pub use netdev::DevicePortEvent;
