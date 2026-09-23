// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::stream::Stream;
use thiserror::Error;

#[cfg_attr(target_os = "linux", path = "usb_linux/mod.rs")]
mod usb_plat;

pub mod bulk_interface;

pub use usb_plat::{
    BulkInEndpoint, BulkOutEndpoint, ControlEndpoint, Interface, InterruptEndpoint,
    IsochronousEndpoint,
};

/// Controls whether a zero-length packet (ZLP) is sent to terminate an OUT transfer
/// that ends on a packet boundary.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ZeroPacket {
    /// Send a zero-length packet if the transfer length is an exact multiple of the endpoint max packet size.
    Send,
    /// Do not send a zero-length packet.
    DoNotSend,
}

/// Selects the bit in USB endpoint addresses that tells us whether it is an in or and out endpoint.
pub(crate) const USB_ENDPOINT_DIR_MASK: u8 = 0x80;

pub struct DeviceHandle(usb_plat::DeviceHandleInner);

impl From<usb_plat::DeviceHandleInner> for DeviceHandle {
    fn from(inner: usb_plat::DeviceHandleInner) -> DeviceHandle {
        DeviceHandle(inner)
    }
}

#[cfg(target_os = "linux")]
impl DeviceHandle {
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Self {
        DeviceHandle(usb_plat::DeviceHandleInner {
            hdl: path.as_ref().to_string_lossy().into_owned(),
            serial: std::sync::OnceLock::new(),
        })
    }
}

impl DeviceHandle {
    /// A printable name for this device.
    pub fn debug_name(&self) -> String {
        self.0.debug_name()
    }

    /// The serial number for the device (if any)
    pub fn serial(&self) -> Option<String> {
        self.0.serial()
    }

    /// Returns the sysfs path for this device if available on the platform.
    pub fn sysfs_path(&self) -> Option<std::path::PathBuf> {
        self.0.sysfs_path()
    }

    /// Returns the negotiated USB connection speed for this device, if available from sysfs.
    pub fn speed(&self) -> Option<UsbSpeed> {
        self.0.speed()
    }
}

/// Negotiated USB connection speed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbSpeed {
    /// Low speed (1.5 Mbps, USB 1.0)
    Low,
    /// Full speed (12 Mbps, USB 1.1)
    Full,
    /// High speed (480 Mbps, USB 2.0)
    High,
    /// SuperSpeed (5 Gbps, USB 3.0)
    Super,
    /// SuperSpeed+ (10 Gbps, USB 3.1)
    SuperPlus,
    /// SuperSpeed+ Gen 2x2 (20 Gbps, USB 3.2)
    SuperPlusBy2,
    /// Other or unknown speed
    Other(String),
}

impl UsbSpeed {
    pub fn from_sysfs_str(s: &str) -> Self {
        match s {
            "1.5" => Self::Low,
            "12" => Self::Full,
            "480" => Self::High,
            "5000" => Self::Super,
            "10000" => Self::SuperPlus,
            "20000" => Self::SuperPlusBy2,
            other => Self::Other(other.to_string()),
        }
    }

    /// Theoretical maximum bus bandwidth in megabytes per second (MB/s).
    pub fn theoretical_max_mbps(&self) -> u32 {
        match self {
            Self::Low => 0,
            Self::Full => 1,
            Self::High => 60,
            Self::Super => 625,
            Self::SuperPlus => 1250,
            Self::SuperPlusBy2 => 2500,
            Self::Other(_) => 0,
        }
    }

    /// Returns true if this connection speed is at least SuperSpeed (USB 3.0, >= 5 Gbps).
    pub fn is_superspeed(&self) -> bool {
        matches!(self, Self::Super | Self::SuperPlus | Self::SuperPlusBy2)
    }
}

impl std::fmt::Display for UsbSpeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "LowSpeed (1.5 Mbps)"),
            Self::Full => write!(f, "FullSpeed (12 Mbps)"),
            Self::High => write!(f, "HighSpeed (480 Mbps, USB 2.0)"),
            Self::Super => write!(f, "SuperSpeed (5 Gbps, USB 3.0)"),
            Self::SuperPlus => write!(f, "SuperSpeed+ (10 Gbps, USB 3.1)"),
            Self::SuperPlusBy2 => write!(f, "SuperSpeed+ (20 Gbps, USB 3.2)"),
            Self::Other(s) => write!(f, "{} Mbps", s),
        }
    }
}

impl DeviceHandle {
    /// Given a path to a USB device, scan each interface available on the device. Each interface's
    /// descriptor is passed to the given callback, and the first descriptor for which the callback
    /// returns `true` will be opened and returned.
    pub fn scan_interfaces(
        &self,
        urb_pool_size: usize,
        f: impl Fn(&DeviceDescriptor, &InterfaceDescriptor) -> bool,
    ) -> Result<Interface> {
        self.0.scan_interfaces(urb_pool_size, f)
    }
}

impl std::fmt::Debug for DeviceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_tuple("DeviceHandle").field(&self.debug_name()).field(&self.serial()).finish()
    }
}

/// Device discovery events. See `wait_for_devices`.
#[derive(Debug)]
pub enum DeviceEvent {
    /// Indicates a new USB device has been plugged in.
    Added(DeviceHandle),
    /// Indicates a USB device has been unplugged.
    Removed(DeviceHandle),
}

/// Errors emitted by USB operations.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Could not write all data (had {0} wrote {1})")]
    ShortWrite(usize, usize),
    #[error("Buffer of size {0} too large for USB API")]
    BufferTooBig(usize),
    #[error("Malformed descriptor table")]
    MalformedDescriptor,
    #[error("Could not find appropriate interface")]
    InterfaceNotFound,
    #[error("IO Error: {0:?}")]
    IOError(#[from] std::io::Error),
    #[error("Discovered device with malformed name: {0}")]
    BadDeviceName(String),
    #[error("Error watching device folder: {0:?}")]
    NotifyError(#[from] notify::Error),
}

/// Descriptive information about a USB device.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DeviceDescriptor {
    pub vendor: u16,
    pub product: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

/// Type of an endpoint on a USB interface.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EndpointType {
    Control,
    Interrupt,
    Isochronous,
    Bulk,
}

/// Direction an Endpoint flows on a USB interface.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EndpointDirection {
    In,
    Out,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InterfaceDescriptor {
    pub id: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub alternate: u8,
    pub endpoints: Vec<EndpointDescriptor>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct EndpointDescriptor {
    pub ty: EndpointType,
    pub address: u8,
}

impl EndpointDescriptor {
    pub fn direction(&self) -> EndpointDirection {
        if (self.address & USB_ENDPOINT_DIR_MASK) != 0 {
            EndpointDirection::In
        } else {
            EndpointDirection::Out
        }
    }
}

/// A USB endpoint. Wraps the four types of endpoint for easy carry.
pub enum Endpoint {
    BulkIn(BulkInEndpoint),
    BulkOut(BulkOutEndpoint),
    Isochronous(IsochronousEndpoint),
    Interrupt(InterruptEndpoint),
    Control(ControlEndpoint),
}

/// Waits for USB devices to appear on the bus.
pub fn wait_for_devices(
    notify_added: bool,
    notify_removed: bool,
) -> Result<impl Stream<Item = Result<DeviceEvent>>> {
    usb_plat::wait_for_devices(notify_added, notify_removed)
}

/// Lists all USB devices currently on the bus.
pub fn enumerate_devices() -> Result<Vec<DeviceHandle>> {
    usb_plat::enumerate_devices()
}

/// Directly finds the USB DeviceHandle for a specific serial number by querying sysfs.
pub fn find_device_by_serial(serial: &str) -> Result<Option<DeviceHandle>> {
    usb_plat::find_device_by_serial(serial)
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
