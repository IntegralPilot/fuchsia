// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::{TargetEvent, TargetHandle};
use fuchsia_async::Task;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::StreamExt;
use notify::event::EventKind;
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// `RecommendedWatcher` is a type alias to FsEvents in the notify crate.
// On macOS this has bugs about what's reported and when regarding
// file removal. Without PollWatcher the watcher would report a fresh file
// as having been deleted even if it is a new file (see https://fxbug.dev/42065810).
#[cfg(target_os = "macos")]
use notify::PollWatcher as RecommendedWatcher;
#[cfg(not(target_os = "macos"))]
use notify::RecommendedWatcher;
// TODO: Extract process liveness checking into a shared host process utility
// library across ffx (e.g. //src/developer/ffx/lib/process) instead of
// duplicating this logic across emulator_instance, pkg, target/discover,
// and discovery.
/// Returns true if the process identified by the pid is running on the host.
pub fn is_pid_running(pid: u32) -> bool {
    if pid != 0 {
        if let Ok(raw_pid) = pid.try_into() {
            let p = nix::unistd::Pid::from_raw(raw_pid);
            // First do a no-hang wait to collect the process if it's defunct.
            let _ = nix::sys::wait::waitpid(p, Some(nix::sys::wait::WaitPidFlag::WNOHANG));
            return nix::sys::signal::kill(p, None).is_ok();
        }
    }
    false
}

/// Trait defining the instance-specific data operations for an [`InstanceWatcher`].
pub trait InstanceSource: Send + Sync + 'static {
    /// Return whether this watcher should watch subdirectories recursively.
    fn recursive(&self) -> bool {
        false
    }

    /// Read all currently existing targets from `instance_root`, returning each target's
    /// unique instance identifier and its target handle.
    fn get_all_targets(&self, root: &Path) -> Vec<(String, TargetHandle)> {
        let mut targets = Vec::new();
        let Ok(entries) = std::fs::read_dir(root) else {
            return targets;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(id) = self.instance_id_from_path(root, &path) {
                if let Some(handle) = self.read_target_handle(root, &path) {
                    targets.push((id, handle));
                }
            }
        }
        targets
    }

    /// Extract the unique instance identifier from a path modified or deleted within `instance_root`.
    /// Returns `None` if the path should be ignored.
    fn instance_id_from_path(&self, root: &Path, path: &Path) -> Option<String>;

    /// Read the target handle for the given instance path.
    /// Returns `Some(handle)` if the instance exists and is running.
    /// Returns `None` if the instance does not exist or has stopped running.
    fn read_target_handle(&self, root: &Path, path: &Path) -> Option<TargetHandle>;
}

/// A generic watcher for directory-based target discovery backends (e.g. emulators, GCE instances).
pub struct InstanceWatcher {
    _drain_task: Task<()>,
}

impl InstanceWatcher {
    pub fn new<S, E>(
        instance_root: PathBuf,
        sender: UnboundedSender<TargetEvent>,
        source: S,
        error_fn: impl Fn(PathBuf, String) -> E,
    ) -> Result<Self, E>
    where
        S: InstanceSource,
    {
        let _ = std::fs::create_dir_all(&instance_root);
        let known: Arc<Mutex<HashMap<String, TargetHandle>>> = Arc::new(Mutex::new(HashMap::new()));

        let existing = source.get_all_targets(&instance_root);
        for (id, handle) in existing {
            known.lock().unwrap().insert(id, handle.clone());
            let _ = sender.unbounded_send(TargetEvent::Added(handle));
        }

        let (event_tx, mut event_rx) = futures::channel::mpsc::channel::<notify::Event>(100);
        let watch_handler = move |res: std::result::Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = event_tx.clone().try_send(event);
            }
        };

        #[cfg(target_os = "macos")]
        let res = RecommendedWatcher::new(
            watch_handler,
            notify::Config::default().with_poll_interval(std::time::Duration::from_millis(500)),
        );
        #[cfg(not(target_os = "macos"))]
        let res = RecommendedWatcher::new(watch_handler, notify::Config::default());

        let mut watcher = res.map_err(|e| error_fn(instance_root.clone(), e.to_string()))?;

        let mode =
            if source.recursive() { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };

        watcher
            .watch(&instance_root, mode)
            .map_err(|e| error_fn(instance_root.clone(), e.to_string()))?;

        let known_clone = Arc::clone(&known);
        let root_clone = instance_root.clone();
        let _drain_task = Task::local(async move {
            let _watcher = watcher;
            while let Some(event) = event_rx.next().await {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in event.paths {
                            if let Some(id) = source.instance_id_from_path(&root_clone, &path) {
                                if let Some(handle) = source.read_target_handle(&root_clone, &path)
                                {
                                    let mut known = known_clone.lock().unwrap();
                                    let prev = known.insert(id, handle.clone());
                                    if prev.as_ref() != Some(&handle) {
                                        let _ = sender.unbounded_send(TargetEvent::Added(handle));
                                    }
                                } else {
                                    let mut known = known_clone.lock().unwrap();
                                    if let Some(handle) = known.remove(&id) {
                                        let _ = sender.unbounded_send(TargetEvent::Removed(handle));
                                    }
                                }
                            }
                        }
                    }
                    EventKind::Remove(_) => {
                        for path in event.paths {
                            if let Some(id) = source.instance_id_from_path(&root_clone, &path) {
                                let mut known = known_clone.lock().unwrap();
                                if let Some(handle) = known.remove(&id) {
                                    let _ = sender.unbounded_send(TargetEvent::Removed(handle));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(Self { _drain_task })
    }
}
