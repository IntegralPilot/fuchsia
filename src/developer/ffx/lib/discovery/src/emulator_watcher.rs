// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TargetEvent;
use crate::error::Error;
use crate::events::TargetHandle;
use crate::instance_watcher::{InstanceSource, InstanceWatcher};
use emulator_instance::EmulatorInstances;
use futures::channel::mpsc::UnboundedSender;
use std::path::{Path, PathBuf};

struct EmulatorSource;

impl InstanceSource for EmulatorSource {
    fn recursive(&self) -> bool {
        true
    }

    fn get_all_targets(&self, root: &Path) -> Vec<(String, TargetHandle)> {
        let emu_instances = EmulatorInstances::new(root.to_path_buf());
        emulator_instance::get_all_targets(&emu_instances)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let name = t.nodename.clone()?;
                let handle = TargetHandle::try_from(t).ok()?;
                Some((name, handle))
            })
            .collect()
    }

    fn instance_id_from_path(&self, root: &Path, path: &Path) -> Option<String> {
        emulator_instance::instance_name_from_path(root, path)
    }

    fn read_target_handle(&self, root: &Path, path: &Path) -> Option<TargetHandle> {
        let name = self.instance_id_from_path(root, path)?;
        let emu_instances = EmulatorInstances::new(root.to_path_buf());
        let info = emulator_instance::get_target(&emu_instances, &name).ok()??;
        TargetHandle::try_from(info).ok()
    }
}

pub struct EmulatorWatcher {
    _watcher: InstanceWatcher,
}

impl EmulatorWatcher {
    pub fn new(
        instance_root: PathBuf,
        sender: UnboundedSender<TargetEvent>,
    ) -> Result<Self, Error> {
        let watcher = InstanceWatcher::new(instance_root, sender, EmulatorSource, |path, err| {
            Error::EmulatorWatcher { path, err }
        })?;
        Ok(Self { _watcher: watcher })
    }
}
