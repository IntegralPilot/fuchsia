// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::{
    EmulatorInstanceData, EmulatorInstanceError, EmulatorInstanceInfo, EmulatorInstances,
    EngineOption, NetworkingMode, Result, SerialMode,
};
use ffx::{TargetAddrInfo, TargetVSockCtx};
use fidl_fuchsia_developer_ffx::{self as ffx, TargetVSockNamespace};
use fidl_fuchsia_net::{IpAddress, Ipv4Address};
use futures::SinkExt;
use futures::channel::mpsc::{self, Receiver, Sender};
use futures::stream::StreamExt;
use notify::event::EventKind::{Create, Modify, Remove};
use notify::event::{CreateKind, Event, RemoveKind};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
// `RecommendedWatcher` is a type alias to FsEvents in the notify crate.
// On mac this seems to have bugs about what's reported and when regarding
// file removal. Without PollWatcher the watcher would report a fresh file
// as having been deleted even if it is a new file.
//
// See https://fxbug.dev/42065810 for details on what happens when using the Default
// RecommendedWatcher (FsEvents).
//
// It's possible that in future versions of this crate this bug will be fixed,
// so it may be worth revisiting this in the future in order to make the code
// in this file a little cleaner and easier to read.
#[cfg(target_os = "macos")]
use notify::PollWatcher as RecommendedWatcher;
#[cfg(not(target_os = "macos"))]
use notify::RecommendedWatcher;
/// Config key to emulator instance data
#[derive(Debug)]
pub struct EmulatorWatcher {
    emu_instance_rx: Receiver<EmulatorInstanceEvent>,
    emu_instance_tx: Sender<EmulatorInstanceEvent>,
    emu_instances: EmulatorInstances,
    // Hold a reference here to the watcher to keep it in scope.
    _watcher: RecommendedWatcher,
}

#[derive(Clone, Debug, PartialEq)]
/// Enum for the payload of emulator instances to check.
/// This is done to allow non-async events from notify::Watcher
/// to be handled as async tasks.
pub(crate) enum EmulatorInstanceEvent {
    Name(String, EventKind),
    Data(Box<EmulatorInstanceData>),
}
/// Action to take for a Target based on an
/// emulator instance. Either Add/Update it
/// or Remove it.
#[derive(Debug, PartialEq)]
pub enum EmulatorTargetAction {
    Add(ffx::TargetInfo),
    Remove(ffx::TargetInfo),
}
#[derive(Debug)]
/// This struct handles the events from the Watcher.
/// Based on the path modified, the emulator instance is
/// determined and added to the event queue to process.
///
struct EmulatorWatcherHandler {
    /// This is the root of the emulator instances.
    instance_dir: PathBuf,
    /// Sender side to send emulator instances to process.
    emu_instance_tx: mpsc::Sender<EmulatorInstanceEvent>,
    /// Based on the instance name, the writing of events to process
    /// is throttled to send no more often than "cutoff".
    throttle: HashMap<String, Instant>,
    cutoff: Duration,
}
pub fn start_emulator_watching(instance_root: PathBuf) -> Result<EmulatorWatcher> {
    let (emu_instance_tx, emu_instance_rx) = mpsc::channel::<EmulatorInstanceEvent>(100);
    if !instance_root.exists() {
        create_dir_all(&instance_root).map_err(|e| EmulatorInstanceError::CreateDirectory {
            path: instance_root.clone(),
            source: e,
        })?;
    }
    let watch_handler = EmulatorWatcherHandler {
        instance_dir: instance_root.clone(),
        emu_instance_tx: emu_instance_tx.clone(),
        cutoff: Duration::from_millis(100),
        throttle: HashMap::new(),
    };
    // Watcher configuration is based on start_socket_watch() in daemon.
    #[cfg(target_os = "macos")]
    let res = RecommendedWatcher::new(
        watch_handler,
        notify::Config::default().with_poll_interval(Duration::from_millis(500)),
    );
    #[cfg(not(target_os = "macos"))]
    let res = RecommendedWatcher::new(watch_handler, notify::Config::default());
    let mut watcher = res.map_err(EmulatorInstanceError::WatcherCreation)?;
    watcher.watch(&instance_root, RecursiveMode::Recursive).map_err(|e| {
        EmulatorInstanceError::WatchDirectory { path: instance_root.clone(), source: e }
    })?;
    let watcher_handler = EmulatorWatcher {
        emu_instances: EmulatorInstances::new(instance_root),
        emu_instance_rx: emu_instance_rx,
        emu_instance_tx,
        _watcher: watcher,
    };
    Ok(watcher_handler)
}
impl EmulatorWatcherHandler {
    // Given a PathBuf, return the name of the emulator instance, if any.
    fn instance_name_from_path<T: AsRef<Path> + std::fmt::Debug>(
        &self,
        instance_path: T,
    ) -> Option<String> {
        let relative = instance_path
            .as_ref()
            .strip_prefix(&self.instance_dir)
            .unwrap_or_else(|_| Path::new(""));
        let mut name: String = "".into();
        if let Some(instance_name) = relative.parent() {
            name = (&instance_name.to_string_lossy()).to_string();
            if name == "" {
                name = (&relative.to_string_lossy()).to_string();
            }
        } else if !relative.to_string_lossy().is_empty() {
            name = (&relative.to_string_lossy()).to_string();
        }
        if !name.is_empty() { Some(name) } else { None }
    }
}
impl notify::EventHandler for EmulatorWatcherHandler {
    fn handle_event(&mut self, event: std::result::Result<notify::Event, notify::Error>) {
        match event {
            Ok(Event { kind: Create(_), paths, .. }) | Ok(Event { kind: Modify(_), paths, .. }) => {
                for p in paths {
                    // Filter out tmp files, these have no extension
                    if let Some(ext) = p.extension() {
                        // Filter out the .log and .serial files
                        // TODO(https://fxbug.dev/42074471): Move emulator logs to ffx log directory
                        if ext == "log" || ext == "serial" {
                            continue;
                        }

                        let now = Instant::now();
                        if let Some(instance_name) = self.instance_name_from_path(&p) {
                            if let Some(last) = self.throttle.get(&instance_name) {
                                if now < *last || now.duration_since(*last) < self.cutoff {
                                    continue;
                                }
                            }
                            log::debug!("triggered by {p:?}");
                            self.throttle.insert(instance_name.clone(), now);
                            let _ = self
                                .emu_instance_tx
                                .try_send(EmulatorInstanceEvent::Name(
                                    instance_name,
                                    Create(CreateKind::Any),
                                ))
                                .map_err(|e| {
                                    log::error!(
                                        "Error sending emulator instance event: {:?} {e:?}",
                                        p
                                    )
                                });
                        }
                    }
                }
            }
            Ok(Event { kind: Remove(RemoveKind::Folder), paths, .. }) => {
                for p in paths {
                    log::debug!("Removal of {p:?} is being processed");
                    if let Some(instance_name) = self.instance_name_from_path(&p) {
                        let _ = self
                            .emu_instance_tx
                            .try_send(EmulatorInstanceEvent::Name(
                                instance_name,
                                Remove(RemoveKind::Folder),
                            ))
                            .map_err(|e| {
                                log::error!("Error sending emulator instance event: {:?} {e:?}", p)
                            });
                    }
                }
            }
            Err(ref e @ notify::Error { ref kind, .. }) => {
                match kind {
                    notify::ErrorKind::Io(ioe) => {
                        log::debug!("IO error. Ignoring {ioe:?}");
                    }
                    _ => {
                        // If we get a non-spurious error, treat that as something that
                        // should cause us to exit.
                        log::warn!("Exiting due to file watcher error: {e:?}");
                    }
                }
            }
            Ok(_) => (),
        }
    }
}
impl EmulatorWatcher {
    /// Returns the action to take with the provided targetInfo for the emulator instance,
    ///  or None if it is not needed.
    pub async fn emulator_target_detected(&mut self) -> Option<EmulatorTargetAction> {
        if let Some(event) = self.emu_instance_rx.next().await {
            log::trace!("checking instance {:?}", event);
            match event {
                EmulatorInstanceEvent::Name(instance_name, kind) => {
                    let instance_dir =
                        match self.emu_instances.get_instance_dir(&instance_name, false) {
                            Ok(d) => d,

                            Err(e) => {
                                log::error!("Error getting instance dir for {instance_name}: {e}");
                                return None;
                            }
                        };
                    match crate::read_from_disk(&instance_dir) {
                        Ok(EngineOption::DoesExist(emu_instance)) => {
                            if let Some(target_info) = Self::handle_instance(&emu_instance) {
                                return Some(EmulatorTargetAction::Add(target_info));
                            } else {
                                return None;
                            }
                        }
                        Ok(EngineOption::DoesNotExist(_)) => {
                            // This usually
                            log::trace!("Emulator instance:{:?} does not exist.", instance_name);
                            // Only remove if the kind is Remove. It is possible to get
                            // DoesNotExist if the json file for the instance is not written completely.
                            if kind == Remove(RemoveKind::Folder) {
                                let target_info = ffx::TargetInfo {
                                    nodename: Some(instance_name),
                                    ..Default::default()
                                };
                                return Some(EmulatorTargetAction::Remove(target_info));
                            }
                        }
                        Err(e) => {
                            log::trace!("Cannot read emulator instance: {e:?}");
                            if kind == Remove(RemoveKind::Folder) {
                                let target_info = ffx::TargetInfo {
                                    nodename: Some(instance_name),
                                    ..Default::default()
                                };
                                return Some(EmulatorTargetAction::Remove(target_info));
                            }
                        }
                    }
                }
                EmulatorInstanceEvent::Data(emu_instance) => {
                    if let Some(target_info) = Self::handle_instance(&emu_instance) {
                        return Some(EmulatorTargetAction::Add(target_info));
                    } else {
                        return None;
                    }
                }
            }
        }
        None
    }
    pub async fn check_all_instances(&mut self) -> Result<()> {
        let instances = self.emu_instances.get_all_instances()?;
        for emu in instances {
            self.emu_instance_tx
                .send(EmulatorInstanceEvent::Data(Box::new(emu)))
                .await
                .map_err(|e| EmulatorInstanceError::SendError(e.to_string()))?;
        }
        Ok(())
    }
    fn handle_instance(instance: &EmulatorInstanceData) -> Option<ffx::TargetInfo> {
        if instance.is_running() {
            log::debug!(
                "Making target from {} using ssh port {:?}",
                instance.get_name(),
                instance.get_ssh_port()
            );
            Self::make_target(&instance)
        } else {
            None
        }
    }
    fn make_target(instance: &EmulatorInstanceData) -> Option<ffx::TargetInfo> {
        let nodename: String = instance.get_name().into();
        let mut addresses = Vec::with_capacity(2);
        let vsock_device =
            instance.emulator_configuration.device.vsock.clone().filter(|x| x.enabled);

        if let Some(v) = &vsock_device {
            addresses.push(TargetAddrInfo::Vsock(TargetVSockCtx {
                cid: v.cid,
                namespace: TargetVSockNamespace::Vsock,
            }));
        }

        if nodename.is_empty() {
            log::debug!("Skipping making target for emulator with empty nodename");
            return None;
        }

        // TUN/TAP emulators are discoverable via mDNS.
        // TODO(435460863): Add unit tests that check this path.
        if instance.get_networking_mode() == &NetworkingMode::Tap && vsock_device.is_none() {
            log::debug!("Skipping making target for {}, since it is tun/tap networking", nodename);
            return None;
        }
        let ssh_port = instance.get_ssh_port();
        let ssh_address = if ssh_port.is_none() {
            if vsock_device.is_none() {
                // No ssh port assigned so don't create a target.
                log::debug!(
                    "Skipping making target for {}, since ssh port and vsock device are both none",
                    nodename
                );
                return None;
            }
            None
        } else {
            // All emulators run on loopback ipv4.
            let ip = IpAddress::Ipv4(Ipv4Address { addr: [127, 0, 0, 1] });
            let loopback = ffx::TargetIpPort { ip, scope_id: 0, port: ssh_port.unwrap() };
            addresses.push(TargetAddrInfo::IpPort(loopback.clone()));
            Some(ffx::TargetIpAddrInfo::IpPort(loopback))
        };

        let serial_number = match &instance.emulator_configuration.runtime.serial_number {
            SerialMode::Enabled(serial) => Some(serial.clone()),
            _ => None,
        };

        Some(ffx::TargetInfo {
            nodename: Some(nodename),
            addresses: Some(addresses),
            serial_number,
            ssh_address,
            ..Default::default()
        })
    }
}

pub fn get_all_targets(instances: &EmulatorInstances) -> Result<Vec<ffx::TargetInfo>> {
    let items = instances.get_all_instances()?;
    Ok(items.iter().flat_map(|i| EmulatorWatcher::make_target(i)).collect())
}

pub fn get_target(instances: &EmulatorInstances, name: &str) -> Result<Option<ffx::TargetInfo>> {
    let instance_dir = instances.get_instance_dir(name, false)?;
    match crate::read_from_disk(&instance_dir) {
        Ok(EngineOption::DoesExist(emu_instance)) => {
            Ok(EmulatorWatcher::handle_instance(&emu_instance))
        }
        _ => Ok(None),
    }
}

pub fn instance_name_from_path(instance_dir: &Path, path: &Path) -> Option<String> {
    if let Some(ext) = path.extension() {
        if ext == "log" || ext == "serial" {
            return None;
        }
    }
    let relative = path.strip_prefix(instance_dir).ok()?;
    let mut name: String = "".into();
    if let Some(instance_name) = relative.parent() {
        name = instance_name.to_string_lossy().to_string();
        if name.is_empty() {
            name = relative.to_string_lossy().to_string();
        }
    } else if !relative.to_string_lossy().is_empty() {
        name = relative.to_string_lossy().to_string();
    }
    if !name.is_empty() { Some(name) } else { None }
}
#[cfg(test)]
mod tests {
    pub(crate) use super::*;
    use crate::EngineState;
    use notify::EventHandler;
    use notify::event::{EventAttributes, ModifyKind};
    use std::process;
    use tempfile::tempdir;
    #[test]
    fn test_instance_name_from_path() -> Result<()> {
        let (emu_instance_tx, _emu_instance_rx) = mpsc::channel::<EmulatorInstanceEvent>(1);
        let temp = tempdir().expect("cannot get tempdir");
        let instance_dir = temp.path().to_path_buf();
        if !instance_dir.exists() {
            create_dir_all(&instance_dir)?;
        }
        let watch_handler = EmulatorWatcherHandler {
            instance_dir: instance_dir.clone(),
            emu_instance_tx: emu_instance_tx,
            cutoff: Duration::from_millis(200),
            throttle: HashMap::new(),
        };
        let test_data = vec![
            (instance_dir.join("emu-instance"), Some(String::from("emu-instance"))),
            (PathBuf::from("/someplace/unknown/emu-instance"), None),
            (PathBuf::from("./emu-instance"), None),
            (PathBuf::from("emu-instance"), None),
            (PathBuf::from("emu-instance"), None),
            (PathBuf::from(""), None),
        ];
        for (p, expected) in test_data {
            let actual = watch_handler.instance_name_from_path(&p);
            assert_eq!(actual, expected, "Calling instance_name_from_path({p:?})");
        }
        Ok(())
    }
    #[fuchsia::test]
    fn test_handle_event() -> Result<()> {
        let temp = tempdir().expect("cannot get tempdir");
        let instance_dir = temp.path().to_path_buf();
        if !instance_dir.exists() {
            create_dir_all(&instance_dir)?;
        }
        let emu_instance_name = String::from("new-emu-instance");
        let new_instance_dir = instance_dir.join(emu_instance_name.clone());
        let new_instance_engine_file = new_instance_dir.join("engine.json");
        let other_file = new_instance_dir.join("some_other_file.dat");
        // notify events
        let test_events = vec![
            // Creating a directory is not enough, there needs to be files in the directory.
            (
                vec![Event {
                    kind: EventKind::Create(CreateKind::Folder),
                    paths: vec![new_instance_dir.clone()],
                    attrs: EventAttributes::new(),
                }],
                vec![None],
            ),
            // Modify a directory, and creating a json file, should emit an event.
            (
                vec![
                    Event {
                        kind: EventKind::Modify(notify::event::ModifyKind::Any),
                        paths: vec![new_instance_dir.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Create(CreateKind::File),
                        paths: vec![new_instance_engine_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                ],
                vec![
                    Some(EmulatorInstanceEvent::Name(
                        emu_instance_name.clone(),
                        Create(CreateKind::Any),
                    )),
                    None,
                ],
            ),
            // Modify a directory, and creating 2 files, should emit just 1 event.
            (
                vec![
                    Event {
                        kind: EventKind::Modify(notify::event::ModifyKind::Any),
                        paths: vec![new_instance_dir.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Create(CreateKind::File),
                        paths: vec![new_instance_engine_file.clone(), other_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                ],
                vec![
                    Some(EmulatorInstanceEvent::Name(
                        emu_instance_name.clone(),
                        Create(CreateKind::Any),
                    )),
                    None,
                ],
            ),
            // Modify a directory, and multiple files created 2 files, should emit just 1 event.
            (
                vec![
                    Event {
                        kind: EventKind::Modify(notify::event::ModifyKind::Any),
                        paths: vec![new_instance_dir.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Create(CreateKind::File),
                        paths: vec![new_instance_engine_file.clone(), other_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Modify(ModifyKind::Any),
                        paths: vec![new_instance_engine_file.clone(), other_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Modify(ModifyKind::Any),
                        paths: vec![new_instance_engine_file.clone(), other_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                ],
                vec![
                    Some(EmulatorInstanceEvent::Name(
                        emu_instance_name.clone(),
                        Create(CreateKind::Any),
                    )),
                    None,
                ],
            ),
            // Modify a directory, and multiple files created 2 files,
            // and then remove everything should emit just 2 events, one for the create/modifies and 1 from the remove
            (
                vec![
                    Event {
                        kind: EventKind::Modify(notify::event::ModifyKind::Any),
                        paths: vec![new_instance_dir.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Create(CreateKind::File),
                        paths: vec![new_instance_engine_file.clone(), other_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Modify(ModifyKind::Any),
                        paths: vec![new_instance_engine_file.clone(), other_file.clone()],
                        attrs: EventAttributes::new(),
                    },
                    Event {
                        kind: EventKind::Remove(RemoveKind::Folder),
                        paths: vec![new_instance_dir.clone()],
                        attrs: EventAttributes::new(),
                    },
                ],
                vec![
                    Some(EmulatorInstanceEvent::Name(
                        emu_instance_name.clone(),
                        Create(CreateKind::Any),
                    )),
                    Some(EmulatorInstanceEvent::Name(
                        emu_instance_name.clone(),
                        Remove(RemoveKind::Folder),
                    )),
                    None,
                ],
            ),
        ];
        for (events, expected) in test_events {
            // create a new watcher and channel.
            let (emu_instance_tx, mut emu_instance_rx) = mpsc::channel::<EmulatorInstanceEvent>(10);
            let mut watch_handler = EmulatorWatcherHandler {
                instance_dir: instance_dir.clone(),
                emu_instance_tx,
                cutoff: Duration::from_millis(100),
                throttle: HashMap::new(),
            };
            for event in &events {
                watch_handler.handle_event(Ok(event.clone()));
            }
            let mut actual_events: Vec<Option<EmulatorInstanceEvent>> = vec![];
            loop {
                let actual_event = match emu_instance_rx.try_next() {
                    Ok(emu_event) => emu_event,
                    // try_next Err() means no messages, but the channel is still open.
                    Err(_) => None,
                };
                actual_events.push(actual_event.clone());
                if actual_event.is_none() {
                    break;
                }
            }
            assert_eq!(actual_events, expected, "for events {events:?}");
        }
        Ok(())
    }
    #[fuchsia::test]
    async fn test_emulator_target_detected() -> Result<()> {
        let temp = tempdir().expect("cannot get tempdir");
        let instance_dir = temp.path().to_path_buf();
        if !instance_dir.exists() {
            create_dir_all(&instance_dir)?;
        }
        let emu_instance_name = String::from("new-emu-instance");
        let new_instance_dir = instance_dir.join(emu_instance_name.clone());
        let _new_instance_engine_file = new_instance_dir.join("engine.json");
        let _other_file = new_instance_dir.join("some_other_file.dat");
        let mut instance_data =
            EmulatorInstanceData::new_with_state("emu-data-instance", EngineState::Running);
        instance_data.set_pid(process::id());
        let mut config = instance_data.get_emulator_configuration_mut();
        config.host.networking = crate::NetworkingMode::User;
        config
            .host
            .port_map
            .insert(String::from("ssh"), crate::PortMapping { guest: 22, host: Some(3322) });
        let mut tap_instance_data = instance_data.clone();
        config = tap_instance_data.get_emulator_configuration_mut();
        config.host.networking = crate::NetworkingMode::Tap;
        let mut serial_instance_data = instance_data.clone();
        serial_instance_data.get_emulator_configuration_mut().runtime.serial_number =
            SerialMode::Enabled("EM-123456789".to_string());
        let ip = IpAddress::Ipv4(Ipv4Address { addr: [127, 0, 0, 1] });
        let loopback =
            ffx::TargetAddrInfo::IpPort(ffx::TargetIpPort { ip, scope_id: 0, port: 3322 });
        let ssh_address =
            Some(ffx::TargetIpAddrInfo::IpPort(ffx::TargetIpPort { ip, scope_id: 0, port: 3322 }));
        // not running
        // not user mode
        // missing reading
        //returns  Option<(ffx::TargetInfo, bool)> {
        let testdata = vec![
            (
                EmulatorInstanceEvent::Name(emu_instance_name.clone(), Remove(RemoveKind::Folder)),
                Some(EmulatorTargetAction::Remove(ffx::TargetInfo {
                    nodename: Some(emu_instance_name.clone()),
                    ..Default::default()
                })),
            ),
            (
                EmulatorInstanceEvent::Data(Box::new(instance_data.clone())),
                Some(EmulatorTargetAction::Add(ffx::TargetInfo {
                    nodename: Some(instance_data.get_name().to_string()),
                    addresses: Some(vec![loopback.clone()]),
                    ssh_address: ssh_address.clone(),
                    ..Default::default()
                })),
            ),
            (
                EmulatorInstanceEvent::Data(Box::new(serial_instance_data.clone())),
                Some(EmulatorTargetAction::Add(ffx::TargetInfo {
                    nodename: Some(serial_instance_data.get_name().to_string()),
                    addresses: Some(vec![loopback]),
                    ssh_address,
                    serial_number: Some("EM-123456789".to_string()),
                    ..Default::default()
                })),
            ),
            (EmulatorInstanceEvent::Data(Box::new(tap_instance_data.clone())), None),
        ];
        for (event, expected) in testdata {
            // create a new watcher and channel.
            let (mut emu_instance_tx, emu_instance_rx) = mpsc::channel::<EmulatorInstanceEvent>(10);
            let watch_handler = EmulatorWatcherHandler {
                instance_dir: instance_dir.clone(),
                emu_instance_tx: emu_instance_tx.clone(),
                cutoff: Duration::from_millis(100),
                throttle: HashMap::new(),
            };
            let iwatcher = RecommendedWatcher::new(
                watch_handler,
                notify::Config::default().with_poll_interval(Duration::from_secs(500 * 60)),
            )?;
            let mut watcher = EmulatorWatcher {
                emu_instances: EmulatorInstances::new(new_instance_dir.clone()),
                emu_instance_rx,
                emu_instance_tx: emu_instance_tx.clone(),
                _watcher: iwatcher,
            };
            emu_instance_tx
                .try_send(event.clone())
                .map_err(|e| EmulatorInstanceError::SendError(e.to_string()))?;
            let actual = watcher.emulator_target_detected().await;
            assert_eq!(expected, actual, "for event {event:?}");
        }
        Ok(())
    }

    #[test]
    fn test_get_all_targets() -> Result<()> {
        use std::fs::File;
        use std::io::Write;
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");

        let instance_root = PathBuf::from(temp_dir.path());
        let emulator_instances = EmulatorInstances::new(instance_root.clone());

        let path1 = instance_root.join("path1");
        create_dir_all(path1.as_path())?;
        let file1_path = path1.join(crate::instances::SERIALIZE_FILE_NAME);
        let mut file1 = File::create(&file1_path)?;
        let mut instance_data = crate::EmulatorInstanceData::new_with_state(
            "emu-data-instance",
            crate::EngineState::Running,
        );
        instance_data.set_pid(std::process::id());
        let config = instance_data.get_emulator_configuration_mut();
        config.host.networking = crate::NetworkingMode::User;
        config.runtime.serial_number = SerialMode::Enabled("EM-123456789".to_string());
        config
            .host
            .port_map
            .insert(String::from("ssh"), crate::PortMapping { guest: 22, host: Some(3322) });
        let emu_config = serde_json::to_string(&instance_data)?;
        file1.write_all(emu_config.as_bytes())?;

        let targets = get_all_targets(&emulator_instances)?;
        assert_eq!(targets.first().unwrap().nodename, Some("emu-data-instance".to_string()));
        assert_eq!(targets.first().unwrap().serial_number, Some("EM-123456789".to_string()));

        Ok(())
    }
}
