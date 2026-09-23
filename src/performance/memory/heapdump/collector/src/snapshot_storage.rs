// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_memory_heapdump_client::StoredSnapshot;
use std::collections::HashMap;
use std::sync::Arc;
use zx::Koid;

use crate::process::Snapshot;

struct Entry {
    process_name: String,
    process_koid: Koid,
    snapshot_name: String,
    snapshot: Arc<dyn Snapshot>,
}

/// Container for stored snapshots.
///
/// Each stored snapshot is identified by a unique ID, which is assigned on insertion.
pub struct SnapshotStorage {
    snapshots: HashMap<u32, Entry>,
    next_id: u32,
}

impl SnapshotStorage {
    pub fn new() -> SnapshotStorage {
        SnapshotStorage { snapshots: HashMap::new(), next_id: 1 }
    }

    pub fn add_snapshot(
        &mut self,
        process_name: String,
        process_koid: Koid,
        snapshot_name: String,
        snapshot: Box<dyn Snapshot>,
    ) -> u32 {
        let id = self.next_id;
        self.snapshots.insert(
            id,
            Entry { process_name, process_koid, snapshot_name, snapshot: snapshot.into() },
        );

        self.next_id += 1;
        id
    }

    pub fn get_snapshot(&self, snapshot_id: u32) -> Option<Arc<dyn Snapshot>> {
        self.snapshots.get(&snapshot_id).map(|entry| entry.snapshot.clone())
    }

    /// Returns the only snapshot with the given name.
    #[cfg(test)]
    pub fn get_snapshot_by_name(
        &self,
        snapshot_name: &str,
    ) -> Result<Arc<dyn Snapshot>, anyhow::Error> {
        let mut matches = self.snapshots.values().filter_map(|entry| {
            if entry.snapshot_name == snapshot_name {
                Some(entry.snapshot.clone())
            } else {
                None
            }
        });
        match (matches.next(), matches.next()) {
            (Some(result), None) => Ok(result),
            (None, _) => anyhow::bail!("No snapshot matches the given name"),
            (Some(_), Some(_)) => anyhow::bail!("More than one snapshot matches the given name"),
        }
    }

    pub fn list_snapshots(&self, filter: impl Fn(Koid, &str) -> bool) -> Vec<StoredSnapshot> {
        self.snapshots
            .iter()
            .filter_map(|(snapshot_id, entry)| {
                if filter(entry.process_koid, &entry.process_name) {
                    Some(StoredSnapshot {
                        snapshot_id: Some(*snapshot_id),
                        snapshot_name: Some(entry.snapshot_name.clone()),
                        process_koid: Some(entry.process_koid.raw_koid()),
                        process_name: Some(entry.process_name.clone()),
                        ..Default::default()
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}
