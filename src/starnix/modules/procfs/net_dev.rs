// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_uapi::errors::Errno;
use std::sync::atomic::Ordering;

#[derive(Clone)]
pub struct ProcNetDev;

impl ProcNetDev {
    pub fn new_node() -> impl FsNodeOps {
        starnix_core::vfs::pseudo::dynamic_file::DynamicFile::new_node(Self)
    }
}

const RX_STAT_NAMES: [&str; 8] = [
    // Receive:
    "bytes   ",
    "packets",
    "errs",
    "drop",
    "fifo",
    "frame",
    "compressed",
    "multicast",
];

const TX_STAT_NAMES: [&str; 8] = [
    // Transmit:
    "bytes   ",
    "packets",
    "errs",
    "drop",
    "fifo",
    "colls",
    "carrier",
    "compressed",
];

impl starnix_core::vfs::pseudo::dynamic_file::DynamicFileSource for ProcNetDev {
    fn generate_locked(
        &self,
        current_task: &CurrentTask,
        sink: &mut starnix_core::vfs::pseudo::dynamic_file::DynamicFileBuf,
    ) -> Result<(), Errno> {
        let (initialized, wq) = &current_task.kernel().netstack_devices.initialized_and_wq;
        // Kick off network worker initialization if it hasn't started yet.
        if !initialized.load(Ordering::SeqCst) {
            let _ = current_task.kernel().network_netlink();
            let waiter = starnix_core::task::Waiter::new();
            let _ = wq.wait_async(&waiter);
            while !initialized.load(Ordering::SeqCst) {
                waiter.wait(current_task)?;
            }
        }

        let devices = current_task.kernel().netstack_devices.snapshot_devices();

        // Headers:
        // Include the separating "|" in the width calculation.
        let rx_width: usize = RX_STAT_NAMES.iter().map(|s| s.len() + 1).sum::<usize>() - 1;
        // Minus 3 for the leading spaces in "|   Receive".
        write!(sink, "Inter-|   {:<width$}|  {}\n", "Receive", "Transmit", width = rx_width - 3)?;
        write!(sink, " face |{}|{}", RX_STAT_NAMES.join(" "), TX_STAT_NAMES.join(" "))?;
        writeln!(sink)?;

        // Per-device stats:
        for (name, _) in devices {
            write!(sink, "{:>6}:", String::from_utf8_lossy(&name))?;
            // TODO(https://fxbug.dev/531661811): Populate with real values.
            // Align to the column name length, with a single space separator.
            let stat_line = RX_STAT_NAMES
                .iter()
                .chain(TX_STAT_NAMES.iter())
                .map(|name| format!("{:>width$}", 0, width = name.len()))
                .collect::<Vec<_>>()
                .join(" ");
            writeln!(sink, "{}", stat_line)?;
        }
        Ok(())
    }
}
