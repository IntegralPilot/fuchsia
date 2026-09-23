// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceMode;
use crate::task::CurrentTask;
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::pseudo::simple_directory::SimpleDirectory;
use crate::vfs::{
    FileObject, FileOps, FsNode, FsNodeOps, FsString, PathBuilder, fileops_impl_noop_sync,
    fileops_impl_seekable, fs_node_impl_not_dir,
};
use starnix_logging::track_stub;
use starnix_rcu::{RcuHashMap, RcuReadScope};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, error};
use std::sync::Arc;

/// A Class is a higher-level view of a device.
///
/// It groups devices based on what they do, rather than how they are connected.
#[derive(Clone)]
pub struct Class {
    pub name: FsString,
    pub dir: Arc<SimpleDirectory>,
    /// Physical bus that the devices belong to.
    pub bus: Bus,
    pub collection: Arc<SimpleDirectory>,
}

impl Class {
    pub fn new(
        name: FsString,
        dir: Arc<SimpleDirectory>,
        bus: Bus,
        collection: Arc<SimpleDirectory>,
    ) -> Self {
        Self { name, dir, bus, collection }
    }
}

impl std::fmt::Debug for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Class").field("name", &self.name).field("bus", &self.bus).finish()
    }
}

/// A Bus identifies how the devices are connected to the processor.
#[derive(Clone)]
pub struct Bus {
    pub name: FsString,
    pub dir: Arc<SimpleDirectory>,
    pub collection: Option<Arc<SimpleDirectory>>,
}

impl Bus {
    pub fn new(
        name: FsString,
        dir: Arc<SimpleDirectory>,
        collection: Option<Arc<SimpleDirectory>>,
    ) -> Self {
        Self { name, dir, collection }
    }
}

impl std::fmt::Debug for Bus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bus").field("name", &self.name).finish()
    }
}

pub type UEventProperties = Vec<(FsString, FsString)>;

#[derive(Clone, Debug)]
pub struct Device {
    pub name: FsString,
    pub class: Class,
    pub metadata: Option<DeviceMetadata>,
}

impl Device {
    pub fn new(name: FsString, class: Class, metadata: Option<DeviceMetadata>) -> Self {
        Self { name, class, metadata }
    }

    /// Returns a path to the device, relative to the sysfs root, going up `depth` directories.
    pub fn path_from_depth(&self, depth: usize) -> FsString {
        let mut builder = PathBuilder::new();
        builder.prepend_element(self.name.as_ref());
        builder.prepend_element(self.class.name.as_ref());
        builder.prepend_element(self.class.bus.name.as_ref());
        builder.prepend_element(b"devices".into());
        for _ in 0..depth {
            builder.prepend_element(b"..".into());
        }
        builder.build_relative()
    }

    pub fn uevent_properties(&self, separator: char) -> FsString {
        let props = self.get_uevent_properties_list();
        flatten_uevent_properties(props, separator)
    }

    pub fn get_uevent_properties_list(&self) -> UEventProperties {
        let mut props = vec![];

        // TODO(https://fxbug.dev/42078277): Pass the synthetic UUID when available.
        // Otherwise, default as "0".
        let path = self.path_from_depth(0);

        let mut devpath = vec![b'/'];
        devpath.extend_from_slice(path.as_ref());

        props.push((b"DEVPATH".into(), devpath.into()));
        props.push((b"SUBSYSTEM".into(), self.class.name.clone()));

        if let Some(metadata) = &self.metadata {
            props.push((b"DEVNAME".into(), metadata.devname.clone()));
            props.push((b"SYNTH_UUID".into(), b"0".into()));
            props.push((b"MAJOR".into(), metadata.devt.major().to_string().into()));
            props.push((b"MINOR".into(), metadata.devt.minor().to_string().into()));
            let scope = RcuReadScope::new();
            for (key, value) in metadata.properties.iter(&scope) {
                props.push((key.clone(), value.clone()));
            }
        }

        props
    }
}

pub fn flatten_uevent_properties(props: UEventProperties, separator: char) -> FsString {
    let mut result = vec![];
    let sep = separator as u8;
    for (key, value) in props {
        result.extend_from_slice(key.as_ref());
        result.push(b'=');
        result.extend_from_slice(value.as_ref());
        result.push(sep);
    }
    result.into()
}

#[derive(Clone, Debug)]
pub struct DeviceMetadata {
    /// Name of the device in /dev.
    ///
    /// Also appears in sysfs via uevent.
    pub devname: FsString,
    pub devt: DeviceId,
    pub mode: DeviceMode,
    pub properties: Arc<RcuHashMap<FsString, FsString>>,
}

impl DeviceMetadata {
    pub fn new(devname: FsString, devt: DeviceId, mode: DeviceMode) -> Self {
        Self { devname, devt, mode, properties: Arc::new(RcuHashMap::default()) }
    }

    pub fn with_devtype(self, devtype: impl Into<FsString>) -> Self {
        self.properties.insert(b"DEVTYPE".into(), devtype.into());
        self
    }
}

pub struct UEventFsNode {
    device: Device,
}

impl UEventFsNode {
    pub fn new(device: Device) -> Self {
        Self { device }
    }
}

impl FsNodeOps for UEventFsNode {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(UEventFile::new(self.device.clone())))
    }
}

struct UEventFile {
    device: Device,
}

impl UEventFile {
    pub fn new(device: Device) -> Self {
        Self { device }
    }

    fn parse_commands(data: &[u8]) -> Vec<&[u8]> {
        data.split(|&c| c == b'\0' || c == b'\n').collect()
    }
}

impl FileOps for UEventFile {
    fileops_impl_seekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let content = self.device.uevent_properties('\n');
        let content_bytes: &[u8] = content.as_ref();
        data.write(content_bytes.get(offset..).ok_or_else(|| errno!(EINVAL))?)
    }

    fn write(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        if offset != 0 {
            return error!(EINVAL);
        }
        let content = data.read_all()?;
        for command in Self::parse_commands(&content) {
            // Ignore empty lines.
            if command == b"" {
                continue;
            }

            match UEventAction::try_from(command) {
                Ok(c) => {
                    current_task.kernel().device_registry.dispatch_uevent(c, self.device.clone())
                }
                Err(e) => {
                    track_stub!(TODO("https://fxbug.dev/297435061"), "synthetic uevent variables");
                    return Err(e);
                }
            }
        }
        Ok(content.len())
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum UEventAction {
    Add,
    Remove,
    Change,
    Move,
    Online,
    Offline,
    Bind,
    Unbind,
}

impl std::fmt::Display for UEventAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UEventAction::Add => write!(f, "add"),
            UEventAction::Remove => write!(f, "remove"),
            UEventAction::Change => write!(f, "change"),
            UEventAction::Move => write!(f, "move"),
            UEventAction::Online => write!(f, "online"),
            UEventAction::Offline => write!(f, "offline"),
            UEventAction::Bind => write!(f, "bind"),
            UEventAction::Unbind => write!(f, "unbind"),
        }
    }
}

impl TryFrom<&[u8]> for UEventAction {
    type Error = Errno;

    fn try_from(action: &[u8]) -> Result<Self, Self::Error> {
        match action {
            b"add" => Ok(UEventAction::Add),
            b"remove" => Ok(UEventAction::Remove),
            b"change" => Ok(UEventAction::Change),
            b"move" => Ok(UEventAction::Move),
            b"online" => Ok(UEventAction::Online),
            b"offline" => Ok(UEventAction::Offline),
            b"bind" => Ok(UEventAction::Bind),
            b"unbind" => Ok(UEventAction::Unbind),
            _ => error!(EINVAL),
        }
    }
}

#[derive(Copy, Clone)]
pub struct UEventContext {
    pub seqnum: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::pseudo::simple_directory::SimpleDirectory;
    use starnix_uapi::device_id::DeviceId;

    #[test]
    fn test_uevent_properties() {
        let dir = SimpleDirectory::new();
        let collection = SimpleDirectory::new();
        let bus = Bus::new("bus".into(), dir.clone(), Some(collection.clone()));
        let class = Class::new("class".into(), dir.clone(), bus, collection);
        let device = Device::new(
            "device".into(),
            class,
            Some(
                DeviceMetadata::new("devname".into(), DeviceId::new(1, 2), DeviceMode::Char)
                    .with_devtype("disk"),
            ),
        );

        assert_eq!(
            device.uevent_properties('\n'),
            b"DEVPATH=/devices/bus/class/device\n\
             SUBSYSTEM=class\n\
             DEVNAME=devname\n\
             SYNTH_UUID=0\n\
             MAJOR=1\n\
             MINOR=2\n\
             DEVTYPE=disk\n"
        );
    }

    #[test]
    fn test_uevent_properties_no_devtype() {
        let dir = SimpleDirectory::new();
        let collection = SimpleDirectory::new();
        let bus = Bus::new("bus".into(), dir.clone(), Some(collection.clone()));
        let class = Class::new("class".into(), dir.clone(), bus, collection);
        let device = Device::new(
            "device".into(),
            class,
            Some(DeviceMetadata::new("devname".into(), DeviceId::new(1, 2), DeviceMode::Char)),
        );

        assert_eq!(
            device.uevent_properties('\n'),
            b"DEVPATH=/devices/bus/class/device\n\
             SUBSYSTEM=class\n\
             DEVNAME=devname\n\
             SYNTH_UUID=0\n\
             MAJOR=1\n\
             MINOR=2\n"
        );
    }

    #[::fuchsia::test]
    fn test_get_uevent_properties_list() {
        let bus = Bus::new("virtual".into(), SimpleDirectory::new(), None);
        let class =
            Class::new("android_usb".into(), SimpleDirectory::new(), bus, SimpleDirectory::new());
        let metadata =
            DeviceMetadata::new("android0".into(), DeviceId::new(1, 2), DeviceMode::Char);
        let device = Device::new("android0".into(), class, Some(metadata));

        let props = device.get_uevent_properties_list();

        // Now we have metadata, so we expect more properties (DEVNAME, SYNTH_UUID, MAJOR, MINOR).
        // Original count was 2 (DEVPATH, SUBSYSTEM).
        // Now we add: DEVNAME, SYNTH_UUID, MAJOR, MINOR. Total 6.
        assert_eq!(props.len(), 6);
        assert_eq!(props[0], ("DEVPATH".into(), "/devices/virtual/android_usb/android0".into()));
        assert_eq!(props[1], ("SUBSYSTEM".into(), "android_usb".into()));

        let properties = &device.metadata.as_ref().unwrap().properties;
        properties.insert("USB_STATE".into(), "CONNECTED".into());
        properties.insert("ABC".into(), "XYZ".into());
        properties.insert("FOO".into(), "BAR".into());

        let mut props = device.get_uevent_properties_list();

        assert_eq!(props.len(), 9);
        // The properties from the metadata HashMap are in non-deterministic order.
        // Sort them by key to make assertions deterministic.
        props[6..].sort();
        assert_eq!(props[6], ("ABC".into(), "XYZ".into()));
        assert_eq!(props[7], ("FOO".into(), "BAR".into()));
        assert_eq!(props[8], ("USB_STATE".into(), "CONNECTED".into()));
    }
}
