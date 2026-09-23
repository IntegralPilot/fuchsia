// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::FsNodeOps;
use crate::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use fidl_fuchsia_power_broker::PowerLevel;
use starnix_logging::{log_info, log_warn};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::borrow::Cow;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum SuspendState {
    /// Suspend-to-disk
    ///
    /// This state offers the greatest energy savings.
    Disk,
    /// Suspend-to-Ram
    ///
    /// This state, if supported, offers significant power savings as everything in the system is
    /// put into a low-power state, except for memory.
    Ram,
    /// Standby
    ///
    /// This state, if supported, offers moderate, but real, energy savings, while providing a
    /// relatively straightforward transition back to the working state.
    ///
    Standby,
    /// Suspend-To-Idle
    ///
    /// This state is a generic, pure software, light-weight, system sleep state.
    Idle,
}

impl std::fmt::Display for SuspendState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SuspendState::Disk => write!(f, "disk"),
            SuspendState::Ram => write!(f, "ram"),
            SuspendState::Standby => write!(f, "standby"),
            SuspendState::Idle => write!(f, "freeze"),
        }
    }
}

impl From<SuspendState> for PowerLevel {
    fn from(value: SuspendState) -> Self {
        match value {
            SuspendState::Disk => 0,
            SuspendState::Ram => 1,
            SuspendState::Standby => 2,
            SuspendState::Idle => 3,
        }
    }
}

pub struct PowerStateFile;

impl PowerStateFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for PowerStateFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let state_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let clean_state_str = state_str.split('\n').next().unwrap_or("");
        let state = match clean_state_str {
            "disk" => SuspendState::Disk,
            "standby" => SuspendState::Standby,
            // TODO(https://fxbug.dev/368394556): Check on mem_file to see what suspend state
            // "mem" represents
            "freeze" | "mem" => SuspendState::Idle,
            _ => return error!(EINVAL),
        };
        let power_manager = &current_task.kernel().suspend_resume_manager;
        let supported_states = power_manager.suspend_states();
        if !supported_states.contains(&state) {
            return error!(EINVAL);
        }
        log_info!(state:?; "Received write to power state file.");
        // LINT.IfChange
        fuchsia_trace::duration!("power", "starnix-sysfs:suspend");
        // LINT.ThenChange(//src/performance/lib/trace_processing/metrics/suspend.py)
        power_manager.suspend(state).inspect_err(|e| log_warn!("Suspend failed: {e}"))?;

        Ok(())
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let states = current_task.kernel().suspend_resume_manager.suspend_states();
        let mut states_array: Vec<String> = states.iter().map(SuspendState::to_string).collect();
        // TODO(https://fxbug.dev/368394556): The “mem” string is interpreted in accordance with
        // the contents of the mem_sleep file.
        states_array.push("mem".to_string());
        states_array.sort();
        let content = states_array.join(" ") + "\n";
        Ok(content.as_bytes().to_owned().into())
    }
}
