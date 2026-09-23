// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::Task;
use futures::StreamExt;
use futures::channel::mpsc::{self, Receiver, Sender};
use notify::EventKind::{Create, Modify, Remove};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions, create_dir_all};
use std::future::Future;
use std::hash::Hash;
use std::io::{BufRead, BufReader};
use std::net::{AddrParseError, SocketAddr};
use std::num::ParseIntError;
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Clone, Copy)]
pub enum FastbootMode {
    TCP,
    UDP,
}

impl std::fmt::Display for FastbootMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::TCP => "tcp",
            Self::UDP => "udp",
        };
        write!(f, "{}", msg)
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum ParseFastbootModeError {
    #[error("Invalid string: \"{}\". Supported: \"tcp\" or \"udp\"", got)]
    InvalidString { got: String },
}

impl FromStr for FastbootMode {
    type Err = ParseFastbootModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tcp" => Ok(Self::TCP),
            "udp" => Ok(Self::UDP),
            e @ _ => Err(ParseFastbootModeError::InvalidString { got: e.to_string() }),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Clone)]
pub struct FastbootEntry {
    mode: FastbootMode,
    socket_addr: SocketAddr,
}

impl FastbootEntry {
    pub fn mode(&self) -> FastbootMode {
        self.mode
    }

    pub fn socket_addr(&self) -> SocketAddr {
        self.socket_addr
    }
}

impl std::fmt::Display for FastbootEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.mode, self.socket_addr)
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum ParseFastbootEntryError {
    #[error("Invalid format: {}", got)]
    InvalidFormat { got: String },
    #[error("Could not parse mode")]
    ModeError(#[from] ParseFastbootModeError),
    #[error("Invalid port number")]
    InvalidPortNum(#[from] ParseIntError),
    #[error("Invalid ip address number")]
    InvalidAddress(#[from] AddrParseError),
}

#[derive(Error, Debug)]
pub enum GetFastbootEntriesError {
    #[error("Invalid entry")]
    InvalidEntry(#[from] ParseFastbootEntryError),
    #[error("Error accessing file")]
    IoError(#[from] std::io::Error),
}

#[derive(Error, Debug)]
pub enum WatcherError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Notify error: {0}")]
    Notify(#[from] notify::Error),
}

impl FromStr for FastbootEntry {
    type Err = ParseFastbootEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (connection_type, addr_port) = s
            .split_once(":")
            .ok_or_else(|| ParseFastbootEntryError::InvalidFormat { got: s.to_string() })?;
        let mode = connection_type.parse::<FastbootMode>()?;

        let socket_addr = SocketAddr::from_str(addr_port)?;

        Ok(FastbootEntry { mode, socket_addr })
    }
}

pub struct FastbootFileWatcher {
    // Task for the drain loop
    drain_task: Option<Task<()>>,
    _watcher: RecommendedWatcher,
}

#[derive(Debug, PartialEq)]
pub enum FastbootEvent {
    Discovered(FastbootEntry),
    Lost(FastbootEntry),
}

pub trait FastbootEventHandler: Send + 'static {
    /// Handles an event.
    fn handle_event(&mut self, event: FastbootEvent) -> impl Future<Output = ()> + Send;
}

impl<F> FastbootEventHandler for F
where
    F: FnMut(FastbootEvent) -> () + Send + 'static,
{
    async fn handle_event(&mut self, x: FastbootEvent) {
        self(x)
    }
}

pub fn recommended_watcher<F>(
    event_handler: F,
    watch_file: impl AsRef<Path>,
) -> Result<FastbootFileWatcher, WatcherError>
where
    F: FastbootEventHandler,
{
    FastbootFileWatcher::new(event_handler, watch_file)
}

impl FastbootFileWatcher {
    pub fn new<F>(event_handler: F, watch_file_path: impl AsRef<Path>) -> Result<Self, WatcherError>
    where
        F: FastbootEventHandler,
    {
        let (sender, receiver) = mpsc::channel::<FastbootEvent>(100);

        match watch_file_path.as_ref().try_exists() {
            Ok(true) => {}
            _ => {
                // First create the parent in case it doesnt exist
                if let Some(parent) = watch_file_path.as_ref().parent() {
                    create_dir_all(parent)?;
                }
                let _file =
                    OpenOptions::new().write(true).create(true).open(watch_file_path.as_ref());
            }
        }

        let handler =
            FastbootFileHandler { fastboot_file_tx: sender, seen_devices: BTreeSet::new() };
        let mut watcher = RecommendedWatcher::new(handler, Config::default())?;

        watcher.watch(watch_file_path.as_ref(), RecursiveMode::NonRecursive)?;

        let mut res = Self { drain_task: None, _watcher: watcher };

        res.drain_task.replace(Task::local(handle_events_loop(receiver, event_handler)));

        Ok(res)
    }
}

pub fn get_fastboot_devices(
    device_file_path: &impl AsRef<Path>,
) -> Result<Vec<FastbootEntry>, GetFastbootEntriesError> {
    match device_file_path.as_ref().try_exists() {
        Ok(false) | Err(_) => {
            return Ok(vec![]);
        }
        Ok(true) => {}
    };
    let file = File::open(&device_file_path)?;
    let lines = BufReader::new(file).lines();
    let mut res = vec![];
    for line in lines.flatten() {
        let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        if line.is_empty() {
            continue;
        }
        let device = line.parse::<FastbootEntry>()?;
        res.push(device);
    }
    Ok(res)
}

#[derive(Debug)]
/// This struct handles the events from the Watcher.
struct FastbootFileHandler {
    /// Sender side to send devices to process.
    fastboot_file_tx: Sender<FastbootEvent>,
    seen_devices: BTreeSet<FastbootEntry>,
}

impl notify::EventHandler for FastbootFileHandler {
    fn handle_event(&mut self, event: Result<notify::Event, notify::Error>) {
        match event {
            Ok(Event { kind: Create(_), paths, .. }) | Ok(Event { kind: Modify(_), paths, .. }) => {
                for p in paths {
                    if p.file_name() == Some(OsStr::new("devices")) {
                        // Cool. Open the file, parse and send events

                        log::warn!("triggered by {p:?}");
                        match File::open(&p) {
                            Err(e) => {
                                log::error!("Error opening fastboot devices file: {:?}: {}", p, e);
                            }
                            Ok(file) => {
                                let lines = BufReader::new(file).lines();

                                for line in lines.flatten() {
                                    let line =
                                        line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
                                    if line.is_empty() {
                                        continue;
                                    }
                                    match line.parse::<FastbootEntry>() {
                                        Err(e) => {
                                            log::error!(
                                                "Error parsing fastboot devices file line: {}. {}",
                                                line,
                                                e
                                            );
                                        }
                                        Ok(device) => {
                                            // TODO(https://fxbug.dev/379733655): Remove this
                                            #[allow(clippy::set_contains_or_insert)]
                                            if !self.seen_devices.contains(&device) {
                                                let _ = self
                                                    .fastboot_file_tx
                                                    .try_send(FastbootEvent::Discovered(
                                                        device.clone(),
                                                    ))
                                                    .map_err(|e| {
                                                        log::error!(
                                                    "Error sending fastboot event: {:?} {e:?}",
                                                    p
                                                )
                                                    });
                                                self.seen_devices.insert(device);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(Event { kind: Remove(_), paths, .. }) => {
                for p in paths {
                    log::debug!("Removal of {p:?} is being processed");
                    if p.file_name() == Some(OsStr::new("devices")) {
                        for device in &self.seen_devices {
                            let _ = self
                                .fastboot_file_tx
                                .try_send(FastbootEvent::Lost(device.clone()))
                                .map_err(|e| {
                                    log::error!("Error sending fastbodt event: {:?} {e:?}", p)
                                });
                        }
                        self.seen_devices.clear();
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
            Ok(..) => (),
        }
    }
}

async fn handle_events_loop<F>(mut receiver: Receiver<FastbootEvent>, mut handler: F)
where
    F: FastbootEventHandler,
{
    loop {
        let event = receiver.next().await.expect("FastbootEvent stream closed?");
        log::trace!("Event loop received event: {:#?}", event);
        handler.handle_event(event).await;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV6};

    #[derive(Debug, thiserror::Error)]
    pub enum TestError {
        #[error("ParseFastbootMode: {0}")]
        ParseMode(#[from] ParseFastbootModeError),

        #[error("ParseFastbootEntry: {0}")]
        ParseEntry(#[from] ParseFastbootEntryError),

        #[error("Watcher: {0}")]
        Watcher(#[from] WatcherError),

        #[error("IO: {0}")]
        Io(#[from] std::io::Error),
    }

    #[fuchsia::test]
    fn test_parse_fastboot_mode() -> Result<(), TestError> {
        let should_be_tcp = "tcp".parse::<FastbootMode>()?;
        assert_eq!(FastbootMode::TCP, should_be_tcp);

        let capitalization_tcp = "tCp".parse::<FastbootMode>();
        assert!(capitalization_tcp.is_err());

        let should_be_udp = "udp".parse::<FastbootMode>()?;
        assert_eq!(FastbootMode::UDP, should_be_udp);

        let capitalization_udp = "UDp".parse::<FastbootMode>();
        assert!(capitalization_udp.is_err());

        assert!("some_string".parse::<FastbootMode>().is_err());

        Ok(())
    }

    #[fuchsia::test]
    fn test_parse_fastboot_entry() -> Result<(), TestError> {
        assert!("tcp:127.0.0.1:81111111111111111111".parse::<FastbootEntry>().is_err());
        assert!("tCp:127.0.0.1:811".parse::<FastbootEntry>().is_err());
        assert!("tCp:totally&not&an&ip&address:811".parse::<FastbootEntry>().is_err());
        assert!("tCp:totally:address:811".parse::<FastbootEntry>().is_err());
        assert!("tCp::811".parse::<FastbootEntry>().is_err());

        assert_eq!(
            Ok(FastbootEntry {
                mode: FastbootMode::TCP,
                socket_addr: SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
                    811,
                    0,
                    1
                ))
            }),
            "tcp:[::1%1]:811".parse::<FastbootEntry>()
        );

        assert_eq!(
            FastbootEntry {
                mode: FastbootMode::TCP,
                socket_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 811),
            },
            "tcp:127.0.0.1:811".parse::<FastbootEntry>()?
        );

        assert_eq!(
            FastbootEntry {
                mode: FastbootMode::UDP,
                socket_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 811),
            },
            "udp:127.0.0.1:811".parse::<FastbootEntry>()?
        );

        Ok(())
    }

    #[fuchsia::test]
    fn test_display_fastboot_entry() -> Result<(), TestError> {
        let entry = FastbootEntry {
            mode: FastbootMode::TCP,
            socket_addr: "127.0.0.1:8080".parse().unwrap(),
        };
        assert_eq!(entry.to_string(), "tcp 127.0.0.1:8080");

        let entry =
            FastbootEntry { mode: FastbootMode::UDP, socket_addr: "[::1]:8080".parse().unwrap() };
        assert_eq!(entry.to_string(), "udp [::1]:8080");
        Ok(())
    }

    #[fuchsia::test]
    fn test_get_fastboot_devices() -> Result<(), TestError> {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new()?;
        writeln!(file, "tcp:127.0.0.1:8080")?;
        writeln!(file, "")?;
        writeln!(file, "\0\0tcp:127.0.0.1:8081\0\0")?;
        writeln!(file, "   ")?;
        writeln!(file, " \0 \0  ")?;
        writeln!(file, "udp:192.168.1.1:9090  ")?;

        let devices = get_fastboot_devices(&file.path()).unwrap();
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].mode, FastbootMode::TCP);
        assert_eq!(devices[0].socket_addr, "127.0.0.1:8080".parse::<SocketAddr>().unwrap());
        assert_eq!(devices[1].mode, FastbootMode::TCP);
        assert_eq!(devices[1].socket_addr, "127.0.0.1:8081".parse::<SocketAddr>().unwrap());
        assert_eq!(devices[2].mode, FastbootMode::UDP);
        assert_eq!(devices[2].socket_addr, "192.168.1.1:9090".parse::<SocketAddr>().unwrap());

        Ok(())
    }
}
