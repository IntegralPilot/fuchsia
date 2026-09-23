// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use futures::stream::Stream;
use notify::Watcher;
use notify::event::{CreateKind, Event, EventKind, ModifyKind};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use super::DeviceHandleInner;
use crate::{DeviceEvent, DeviceHandle, Error, Result};

/// Root of the Linux devfs
static DEV_DIR: &str = "/dev";
/// Bus device folder within the Linux devfs
static DEV_BUS_DIR: &str = "/dev/bus";
/// The USB bus folder within the Linux devfs
static USB_FS_DIR: &str = "/dev/bus/usb";

/// A stream of new devices as they appear on the bus. See [`wait_for_devices`].
pub struct DeviceStream {
    /// Watches the /dev filesystem for new USB devices.
    watcher: notify::RecommendedWatcher,
    /// This is where results from `watcher` are published.
    queue: UnboundedReceiver<notify::Result<Event>>,
    /// Whether we want to get Added events.
    notify_added: bool,
    /// Whether we want to get Removed events.
    notify_removed: bool,
    /// Paths which we've seen before but which weren't readable at the time. If they become
    /// readable we can emit events for them.
    waiting_readable: Vec<PathBuf>,
}

/// Wrapper around [`std::fs::read_dir`] that returns paths rather than `DirEntry` structs and will not
/// return any path containing a dot (or as a side effect, any path which cannot be converted to
/// UTF-8 to check for dots).
fn read_dir_path_no_dots<P: AsRef<Path>>(
    path: P,
) -> Result<impl std::iter::Iterator<Item = std::io::Result<std::path::PathBuf>>> {
    Ok(std::fs::read_dir(path)?.filter_map(|x| {
        x.map(|entry| {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|x| x.to_str()) else {
                return None;
            };

            if name.contains('.') {
                return None;
            }

            return Some(path);
        })
        .transpose()
    }))
}

/// Determine whether a file at the given path is readable.
///
/// The Rust standard file APIs aren't granular enough to do this, and the docs for
/// [`std::fs::Permissions`] specifically suggest using `libc::access` if we need more.
fn readable<P: AsRef<Path>>(path: P) -> bool {
    nix::unistd::access(path.as_ref(), nix::unistd::AccessFlags::R_OK).is_ok()
}

/// Lists all USB devices currently on the bus.
pub fn enumerate_devices() -> Result<Vec<DeviceHandle>> {
    let mut devices: Vec<DeviceHandle> = vec![];
    for path in read_dir_path_no_dots(USB_FS_DIR)? {
        let path = path?;
        if path.is_dir() {
            for path in read_dir_path_no_dots(path)? {
                let path = path?;
                let full_path = path
                    .to_str()
                    .ok_or_else(|| Error::BadDeviceName(path.to_string_lossy().to_string()))?
                    .to_owned();
                devices.push(DeviceHandleInner::new(full_path).into());
            }
        }
    }
    Ok(devices)
}

/// Directly finds the USB DeviceHandle for a specific serial number by querying sysfs.
pub fn find_device_by_serial(serial: &str) -> Result<Option<DeviceHandle>> {
    use std::fs::{metadata, read_to_string};
    use std::os::unix::fs::MetadataExt;
    use std::str::FromStr;

    fn file_val<F: FromStr>(path: &Path, filename: &str) -> Option<F> {
        let val = read_to_string(path.join(filename)).ok()?;
        val.trim().parse::<F>().ok()
    }

    let Ok(entries) = std::fs::read_dir("/sys/bus/usb/devices") else {
        return Ok(None);
    };

    for path in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.contains(':')
        {
            continue;
        }

        let serial_path = path.join("serial");
        if let Ok(dev_serial) = read_to_string(serial_path)
            && dev_serial.trim() == serial
        {
            // Fast path: find the device directly from busnum and devnum attributes
            if let (Some(busnum), Some(devnum)) =
                (file_val::<u32>(&path, "busnum"), file_val::<u32>(&path, "devnum"))
            {
                let dev_path = format!("{}/{:03}/{:03}", USB_FS_DIR, busnum, devnum);
                return Ok(Some(DeviceHandleInner::new(dev_path).into()));
            }

            // Fallback: parse dev file major:minor and match against enumerated devices
            if let Ok(content) = read_to_string(path.join("dev"))
                && let Some((major_str, minor_str)) = content.trim().split_once(':')
            {
                if let (Ok(major), Ok(minor)) = (major_str.parse::<u32>(), minor_str.parse::<u32>())
                {
                    let devices = enumerate_devices()?;
                    for dev in devices {
                        if metadata(&dev.debug_name())
                            .map(|m| (libc::major(m.rdev()), libc::minor(m.rdev())))
                            .ok()
                            == Some((major, minor))
                        {
                            return Ok(Some(dev));
                        }
                    }
                }
            }
        }
    }

    Ok(None)
}

/// Waits for USB devices to appear on the bus.
pub fn wait_for_devices(notify_added: bool, notify_removed: bool) -> Result<DeviceStream> {
    let (sender, queue) = unbounded();
    let force_sender = sender.clone();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = sender.unbounded_send(res);
    })?;

    if Path::new(DEV_BUS_DIR).is_dir() {
        force_sender
            .unbounded_send(Ok(Event {
                kind: EventKind::Create(CreateKind::Folder),
                paths: vec![Path::new(DEV_BUS_DIR).to_path_buf()],
                attrs: Default::default(),
            }))
            .expect("Receiver should still be in scope!");

        if Path::new(USB_FS_DIR).is_dir() {
            force_sender
                .unbounded_send(Ok(Event {
                    kind: EventKind::Create(CreateKind::Folder),
                    paths: vec![Path::new(USB_FS_DIR).to_path_buf()],
                    attrs: Default::default(),
                }))
                .expect("Receiver should still be in scope!");

            for path in read_dir_path_no_dots(USB_FS_DIR)? {
                let path = path?;
                if path.is_dir() {
                    for path in read_dir_path_no_dots(path)? {
                        let path = path?;
                        force_sender
                            .unbounded_send(Ok(Event {
                                // Might want to figure out what the actual kind is?
                                kind: EventKind::Create(CreateKind::File),
                                paths: vec![path],
                                attrs: Default::default(),
                            }))
                            .expect("Receiver should still be in scope!");
                    }
                }
            }
        }
    }

    watcher.watch(Path::new(DEV_DIR), notify::RecursiveMode::NonRecursive)?;

    Ok(DeviceStream { watcher, queue, notify_added, notify_removed, waiting_readable: Vec::new() })
}

impl Stream for DeviceStream {
    type Item = Result<DeviceEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(event) = ready!(Pin::new(&mut self.queue).poll_next(cx)) else {
                return Poll::Ready(None);
            };

            let event = event?;

            let dev_bus_dir = Path::new(DEV_BUS_DIR);
            let usb_fs_dir = Path::new(USB_FS_DIR);

            if event.paths.iter().any(|x| x == dev_bus_dir) {
                if event.kind == EventKind::Create(CreateKind::Folder) {
                    self.watcher.watch(dev_bus_dir, notify::RecursiveMode::NonRecursive)?;
                } else if matches!(event.kind, EventKind::Remove(_)) {
                    self.watcher.unwatch(dev_bus_dir)?;
                }
            } else if event.paths.iter().any(|x| x == usb_fs_dir) {
                if event.kind == EventKind::Create(CreateKind::Folder) {
                    self.watcher.watch(usb_fs_dir, notify::RecursiveMode::Recursive)?;
                } else if matches!(event.kind, EventKind::Remove(_)) {
                    self.watcher.unwatch(usb_fs_dir)?;
                }
            } else if let Some((full_path, path)) = event
                .paths
                .iter()
                .filter_map(|x| x.strip_prefix(usb_fs_dir).ok().map(|y| (x, y)))
                .next()
            {
                let mut components = path.components();
                if !matches!(components.next(), Some(Component::Normal(_))) {
                    continue;
                }
                if !matches!(components.next(), Some(Component::Normal(_))) {
                    continue;
                }
                if !matches!(components.next(), None) {
                    continue;
                }

                if self.notify_added {
                    let adding = if self.waiting_readable.iter().any(|x| x == full_path)
                        && matches!(event.kind, EventKind::Modify(ModifyKind::Metadata(_)))
                    {
                        self.waiting_readable.retain(|x| x != full_path);
                        true
                    } else {
                        matches!(event.kind, EventKind::Create(_))
                    };

                    if adding {
                        if readable(full_path) {
                            let path = full_path
                                .to_str()
                                .ok_or_else(|| {
                                    Error::BadDeviceName(path.to_string_lossy().to_string())
                                })?
                                .to_owned();
                            return Poll::Ready(Some(Ok(DeviceEvent::Added(
                                DeviceHandleInner::new(path).into(),
                            ))));
                        } else {
                            self.waiting_readable.push(full_path.clone());
                            continue;
                        }
                    }
                }

                if matches!(event.kind, EventKind::Remove(_)) {
                    self.waiting_readable.retain(|x| x != full_path);
                    if self.notify_removed {
                        let path = full_path
                            .to_str()
                            .ok_or_else(|| {
                                Error::BadDeviceName(path.to_string_lossy().to_string())
                            })?
                            .to_owned();
                        return Poll::Ready(Some(Ok(DeviceEvent::Removed(
                            DeviceHandleInner::new(path).into(),
                        ))));
                    }
                }
            }
        }
    }
}
