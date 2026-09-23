// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_fastboot::common::caching::CachingFastboot;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use ffx_fastboot_interface::fastboot_proxy::FastbootProxy;
use ffx_fastboot_interface::interface_factory::InterfaceFactoryBase;
use ffx_fastboot_transport_factory::tcp::TcpFactory;
use ffx_fastboot_transport_factory::udp::UdpFactory;
use ffx_fastboot_transport_factory::usb::UsbFactory;
use ffx_fastboot_transport_interface::tcp::TcpNetworkInterface;
use ffx_fastboot_transport_interface::udp::UdpNetworkInterface;
use netext::MultithreadedTokioAsyncWrapper;
use std::net::SocketAddr;
use std::path::PathBuf;
use thiserror::Error;
use tokio::net::TcpStream;
use usb_fastboot_discovery::Interface as AsyncInterface;

#[derive(Error, Debug)]
pub enum ConnectionFactoryError {
    #[error("Could not get a valid TCP address for target")]
    NoTcpAddress,

    #[error("Could not get a valid UDP address for target")]
    NoUdpAddress,

    #[error("USB Interface creation error for serial {serial}: {source}")]
    UsbInterfaceCreation {
        serial: String,
        source: ffx_fastboot_interface::interface_factory::InterfaceFactoryError,
    },

    #[error("TCP Interface creation error for address {addr}: {source}")]
    TcpInterfaceCreation {
        addr: std::net::SocketAddr,
        source: ffx_fastboot_interface::interface_factory::InterfaceFactoryError,
    },

    #[error("UDP Interface creation error for address {addr}: {source}")]
    UdpInterfaceCreation {
        addr: std::net::SocketAddr,
        source: ffx_fastboot_interface::interface_factory::InterfaceFactoryError,
    },
}

pub enum FastbootConnectionKind {
    Usb(String),
    Tcp(String, SocketAddr),
    Udp(String, SocketAddr),
}

#[async_trait(?Send)]
pub trait FastbootConnectionFactory {
    async fn build_interface(
        &self,
        connection: FastbootConnectionKind,
    ) -> Result<Box<dyn FastbootInterface>, ConnectionFactoryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryLimit {
    Default,
    Forever,
    Count(u64),
}

pub struct ConnectionFactory {
    context: EnvironmentContext,
    retry_limit: RetryLimit,
}

impl ConnectionFactory {
    pub fn new(context: &EnvironmentContext) -> Self {
        Self { context: context.clone(), retry_limit: RetryLimit::Default }
    }

    pub fn with_retry_limit(mut self, retry_limit: RetryLimit) -> Self {
        self.retry_limit = retry_limit;
        self
    }
}

#[async_trait(?Send)]
impl FastbootConnectionFactory for ConnectionFactory {
    async fn build_interface(
        &self,
        connection: FastbootConnectionKind,
    ) -> Result<Box<dyn FastbootInterface>, ConnectionFactoryError> {
        match connection {
            FastbootConnectionKind::Usb(serial_number) => {
                Ok(Box::new(CachingFastboot::new(usb_proxy(serial_number).await?)))
            }
            FastbootConnectionKind::Tcp(target_name, addr) => {
                let config = match self.retry_limit {
                    RetryLimit::Default => FastbootNetworkConnectionConfig::new_tcp(&self.context),
                    RetryLimit::Forever => FastbootNetworkConnectionConfig::forever(),
                    RetryLimit::Count(n) => {
                        let mut config = FastbootNetworkConnectionConfig::new_tcp(&self.context);
                        config.retry_count = Some(n);
                        config
                    }
                };
                let fastboot_device_file_path: Option<PathBuf> =
                    self.context.get(ffx_config::keys::FASTBOOT_FILE_PATH).ok();
                Ok(Box::new(CachingFastboot::new(
                    tcp_proxy(&self.context, target_name, fastboot_device_file_path, &addr, config)
                        .await?,
                )))
            }
            FastbootConnectionKind::Udp(target_name, addr) => {
                let config = match self.retry_limit {
                    RetryLimit::Default => FastbootNetworkConnectionConfig::new_udp(&self.context),
                    RetryLimit::Forever => FastbootNetworkConnectionConfig::forever(),
                    RetryLimit::Count(n) => {
                        let mut config = FastbootNetworkConnectionConfig::new_udp(&self.context);
                        config.retry_count = Some(n);
                        config
                    }
                };
                let fastboot_device_file_path: Option<PathBuf> =
                    self.context.get(ffx_config::keys::FASTBOOT_FILE_PATH).ok();
                Ok(Box::new(CachingFastboot::new(
                    udp_proxy(&self.context, target_name, fastboot_device_file_path, &addr, config)
                        .await?,
                )))
            }
        }
    }
}

const UDP_RETRY_COUNT: &str = "fastboot.network.udp.retry_count";
const UDP_RETRY_COUNT_DEFAULT: u64 = 3;
const UDP_WAIT_SECONDS: &str = "fastboot.network.udp.retry_wait_seconds";
const UDP_WAIT_SECONDS_DEFAULT: u64 = 2;
const TCP_RETRY_COUNT: &str = "fastboot.network.tcp.retry_count";
const TCP_RETRY_COUNT_DEFAULT: u64 = 3;
const TCP_WAIT_SECONDS: &str = "fastboot.network.tcp.retry_wait_seconds";
const TCP_WAIT_SECONDS_DEFAULT: u64 = 2;

pub struct FastbootNetworkConnectionConfig {
    retry_wait_seconds: u64,
    retry_count: Option<u64>,
}

impl FastbootNetworkConnectionConfig {
    pub fn new(retry_wait_seconds: u64, retry_count: Option<u64>) -> Self {
        Self { retry_wait_seconds, retry_count }
    }

    fn new_from_config(
        context: &EnvironmentContext,
        retry_key: &str,
        retry_default: u64,
        wait_key: &str,
        wait_default: u64,
    ) -> Self {
        let retry_count = context.get(retry_key).unwrap_or(retry_default);
        let retry_wait_seconds = context.get(wait_key).unwrap_or(wait_default);
        Self::new(retry_wait_seconds, Some(retry_count))
    }

    pub fn forever() -> Self {
        Self { retry_wait_seconds: 2, retry_count: None }
    }

    pub fn new_tcp(context: &EnvironmentContext) -> Self {
        Self::new_from_config(
            context,
            TCP_RETRY_COUNT,
            TCP_RETRY_COUNT_DEFAULT,
            TCP_WAIT_SECONDS,
            TCP_WAIT_SECONDS_DEFAULT,
        )
    }

    pub fn new_udp(context: &EnvironmentContext) -> Self {
        Self::new_from_config(
            context,
            UDP_RETRY_COUNT,
            UDP_RETRY_COUNT_DEFAULT,
            UDP_WAIT_SECONDS,
            UDP_WAIT_SECONDS_DEFAULT,
        )
    }
}

///////////////////////////////////////////////////////////////////////////////
// AsyncInterface
//

/// Creates a FastbootProxy over USB for a device with the given serial number
async fn usb_proxy(
    serial_number: String,
) -> Result<FastbootProxy<AsyncInterface>, ConnectionFactoryError> {
    let mut interface_factory = UsbFactory::new(serial_number.clone());
    let interface = interface_factory.open().await.map_err(|e| {
        ConnectionFactoryError::UsbInterfaceCreation { serial: serial_number.clone(), source: e }
    })?;

    Ok(FastbootProxy::<AsyncInterface>::new(serial_number, interface, interface_factory))
}

///////////////////////////////////////////////////////////////////////////////
// TcpInterface
//

/// Creates a FastbootProxy over TCP for a device at the given SocketAddr
async fn tcp_proxy(
    context: &EnvironmentContext,
    target_name: String,
    fastboot_device_file_path: Option<PathBuf>,
    addr: &SocketAddr,
    config: FastbootNetworkConnectionConfig,
) -> Result<
    FastbootProxy<TcpNetworkInterface<MultithreadedTokioAsyncWrapper<TcpStream>>>,
    ConnectionFactoryError,
> {
    let mut factory = TcpFactory::new(
        context,
        target_name,
        fastboot_device_file_path,
        *addr,
        config.retry_count,
        config.retry_wait_seconds,
    );
    let interface = factory
        .open()
        .await
        .map_err(|e| ConnectionFactoryError::TcpInterfaceCreation { addr: *addr, source: e })?;
    Ok(FastbootProxy::<TcpNetworkInterface<MultithreadedTokioAsyncWrapper<TcpStream>>>::new(
        addr.to_string(),
        interface,
        factory,
    ))
}

///////////////////////////////////////////////////////////////////////////////
// UdpInterface
//

/// Creates a FastbootProxy over UDP for a device at the given SocketAddr
async fn udp_proxy(
    context: &EnvironmentContext,
    target_name: String,
    fastboot_device_file_path: Option<PathBuf>,
    addr: &SocketAddr,
    config: FastbootNetworkConnectionConfig,
) -> Result<FastbootProxy<UdpNetworkInterface>, ConnectionFactoryError> {
    let mut factory = UdpFactory::new(
        context,
        target_name,
        fastboot_device_file_path,
        *addr,
        config.retry_count,
        config.retry_wait_seconds,
    );
    let interface = factory
        .open()
        .await
        .map_err(|e| ConnectionFactoryError::UdpInterfaceCreation { addr: *addr, source: e })?;
    Ok(FastbootProxy::<UdpNetworkInterface>::new(addr.to_string(), interface, factory))
}

/// Creates a FastbootInterface based on the provided target state
pub async fn get_fastboot_interface(
    fastboot_state: &discovery::FastbootTargetState,
    node_name: Option<String>,
    context: &EnvironmentContext,
    retry_limit: RetryLimit,
) -> Result<Box<dyn FastbootInterface>, ConnectionFactoryError> {
    let connection_kind = get_connection_kind(fastboot_state, node_name)?;
    let factory = ConnectionFactory::new(context).with_retry_limit(retry_limit);
    factory.build_interface(connection_kind).await
}

pub fn get_connection_kind(
    fastboot_state: &discovery::FastbootTargetState,
    node_name: Option<String>,
) -> Result<FastbootConnectionKind, ConnectionFactoryError> {
    let connection_kind = match fastboot_state.connection_state {
        discovery::FastbootConnectionState::Usb => {
            FastbootConnectionKind::Usb(fastboot_state.serial_number.clone())
        }
        // This assumes the first address in the array will a.) exist, and b.) be the _most
        // correct_ address from which we're selecting.
        discovery::FastbootConnectionState::Tcp(ref v) => {
            let Some(addr) = v.first() else {
                return Err(ConnectionFactoryError::NoTcpAddress);
            };
            let socket_addr: SocketAddr = addr.into();
            let name = get_fallback_name(node_name, &socket_addr, "TCP");
            FastbootConnectionKind::Tcp(name, socket_addr)
        }
        discovery::FastbootConnectionState::Udp(ref v) => {
            let Some(addr) = v.first() else {
                return Err(ConnectionFactoryError::NoUdpAddress);
            };
            let socket_addr: SocketAddr = addr.into();
            let name = get_fallback_name(node_name, &socket_addr, "UDP");
            FastbootConnectionKind::Udp(name, socket_addr)
        }
    };
    Ok(connection_kind)
}

fn get_fallback_name(
    node_name: Option<String>,
    socket_addr: &SocketAddr,
    protocol: &str,
) -> String {
    match node_name {
        Some(name) => name,
        None => {
            log::debug!(
                "node_name is missing for {} target, rediscovery will be impossible. Using address {} as fallback.",
                protocol,
                socket_addr
            );
            socket_addr.to_string()
        }
    }
}

pub mod test {
    use super::*;
    use ffx_fastboot_interface::test::{FakeServiceCommands, TestFastbootInterface};
    use std::sync::{Arc, Mutex};

    pub struct TestConnectionFactory {
        state: Arc<Mutex<FakeServiceCommands>>,
    }

    #[async_trait(?Send)]
    impl FastbootConnectionFactory for TestConnectionFactory {
        async fn build_interface(
            &self,
            _connection: FastbootConnectionKind,
        ) -> Result<Box<dyn FastbootInterface>, ConnectionFactoryError> {
            Ok(Box::new(TestFastbootInterface::new(self.state.clone())))
        }
    }

    pub fn setup_connection_factory()
    -> (Arc<Mutex<FakeServiceCommands>>, impl FastbootConnectionFactory) {
        let state = Arc::new(Mutex::new(FakeServiceCommands::default()));
        (state.clone(), TestConnectionFactory { state: state })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discovery::{FastbootConnectionState, FastbootTargetState};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_get_connection_kind_usb() {
        let serial = "serial123".to_string();
        let state = FastbootTargetState {
            serial_number: serial.clone(),
            connection_state: FastbootConnectionState::Usb,
        };
        let kind = get_connection_kind(&state, None).unwrap();
        match kind {
            FastbootConnectionKind::Usb(s) => assert_eq!(s, serial),
            _ => panic!("Expected Usb connection kind"),
        }
    }

    #[test]
    fn test_get_connection_kind_tcp() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let state = FastbootTargetState {
            serial_number: "serial".to_string(),
            connection_state: FastbootConnectionState::Tcp(vec![addr.into()]),
        };
        let kind = get_connection_kind(&state, Some("node".to_string())).unwrap();
        match kind {
            FastbootConnectionKind::Tcp(n, a) => {
                assert_eq!(n, "node");
                assert_eq!(a, addr);
            }
            _ => panic!("Expected Tcp connection kind"),
        }
    }

    #[test]
    fn test_get_connection_kind_udp() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let state = FastbootTargetState {
            serial_number: "serial".to_string(),
            connection_state: FastbootConnectionState::Udp(vec![addr.into()]),
        };
        let kind = get_connection_kind(&state, None).unwrap();
        match kind {
            FastbootConnectionKind::Udp(n, a) => {
                assert_eq!(n, addr.to_string());
                assert_eq!(a, addr);
            }
            _ => panic!("Expected Udp connection kind"),
        }
    }

    #[test]
    fn test_get_connection_kind_tcp_fallback() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let state = FastbootTargetState {
            serial_number: "serial".to_string(),
            connection_state: FastbootConnectionState::Tcp(vec![addr.into()]),
        };
        let kind = get_connection_kind(&state, None).unwrap();
        match kind {
            FastbootConnectionKind::Tcp(n, a) => {
                assert_eq!(n, addr.to_string());
                assert_eq!(a, addr);
            }
            _ => panic!("Expected Tcp connection kind"),
        }
    }

    #[test]
    fn test_get_connection_kind_empty_tcp() {
        let state = FastbootTargetState {
            serial_number: "serial".to_string(),
            connection_state: FastbootConnectionState::Tcp(vec![]),
        };
        assert!(get_connection_kind(&state, None).is_err());
    }

    #[test]
    fn test_get_connection_kind_empty_udp() {
        let state = FastbootTargetState {
            serial_number: "serial".to_string(),
            connection_state: FastbootConnectionState::Udp(vec![]),
        };
        assert!(get_connection_kind(&state, None).is_err());
    }
}
