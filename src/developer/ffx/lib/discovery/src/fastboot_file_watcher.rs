// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TargetEvent;
use crate::error::Error;
use fastboot_file_discovery::{FastbootFileWatcher, get_fastboot_devices};
use futures::channel::mpsc::UnboundedSender;
use std::path::PathBuf;

pub struct FastbootWatcher {
    // Task for the drain loop
    _watcher: FastbootFileWatcher,
}

impl FastbootWatcher {
    pub fn new(
        instance_root: PathBuf,
        sender: UnboundedSender<TargetEvent>,
    ) -> Result<Self, Error> {
        let existing = get_fastboot_devices(&instance_root)
            .map_err(|err| Error::FastbootDiscovery { path: instance_root.clone(), err })?;
        for device in existing {
            let event = fastboot_file_discovery::FastbootEvent::Discovered(device);
            let handle = event.into();
            let _ = sender.unbounded_send(handle);
        }

        // FastbootFile (and therefore notify thread) lifetime should last as long as the task,
        // because it is moved into the loop
        let watcher = fastboot_file_discovery::recommended_watcher(
            move |res: fastboot_file_discovery::FastbootEvent| {
                // Translate the result to a TargetEvent
                log::trace!("discovery watcher got fastboot file event: {:#?}", res);
                let event = res.into();
                let _ = sender.unbounded_send(event);
            },
            instance_root.clone(),
        )
        .map_err(|e| Error::FastbootWatcher { path: instance_root, err: e.to_string() })?;
        Ok(Self { _watcher: watcher })
    }
}
