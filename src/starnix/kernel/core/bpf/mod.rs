// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of (e)BPF.
//!
//! BPF stands for Berkeley Packet Filter and is an API introduced in BSD that allows filtering
//! network packets by running little programs in the kernel. eBPF stands for extended BFP and
//! is a Linux extension of BPF that allows hooking BPF programs into many different
//! non-networking-related contexts.

pub mod attachments;
pub mod context;
pub mod fs;
pub mod map;
pub mod program;
pub mod syscalls;

use crate::bpf::attachments::EbpfAttachments;
use crate::bpf::map::{BpfMapHandle, BpfMapId, WeakBpfMapHandle};
use crate::bpf::program::{ProgramHandle, ProgramId, WeakProgramHandle};
use ebpf::MapFlags;
use starnix_sync::{EbpfStateLock, LockDepMutex};
use starnix_uapi::{bpf_map_type, bpf_map_type_BPF_MAP_TYPE_SK_STORAGE};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;
use zerocopy::IntoBytes as _;

struct WeakMapWithType {
    map_type: bpf_map_type,
    weak_map: WeakBpfMapHandle,
}

impl WeakMapWithType {
    fn new(map: &BpfMapHandle) -> Self {
        Self { map_type: map.schema.map_type, weak_map: Arc::downgrade(map) }
    }
}

/// Stores global eBPF state.
#[derive(Default)]
pub struct EbpfState {
    pub attachments: EbpfAttachments,

    programs: LockDepMutex<BTreeMap<ProgramId, WeakProgramHandle>, EbpfStateLock>,
    maps: LockDepMutex<BTreeMap<BpfMapId, WeakMapWithType>, EbpfStateLock>,
}

impl EbpfState {
    fn register_program(&self, program: &ProgramHandle) {
        self.programs.lock().insert(program.id(), Arc::downgrade(program));
    }

    fn unregister_program(&self, id: ProgramId) {
        self.programs.lock().remove(&id).expect("Missing eBPF program");
    }

    fn get_next_program_id(&self, start_id: ProgramId) -> Option<ProgramId> {
        self.programs
            .lock()
            .range((Bound::Excluded(start_id), Bound::Unbounded))
            .next()
            .map(|(k, _)| *k)
    }

    fn get_program_by_id(&self, id: ProgramId) -> Option<ProgramHandle> {
        self.programs.lock().get(&id).map(|p| p.upgrade()).flatten()
    }

    fn register_map(&self, map: &BpfMapHandle) {
        self.maps.lock().insert(map.id(), WeakMapWithType::new(map));
    }

    fn unregister_map(&self, id: BpfMapId) {
        self.maps.lock().remove(&id).expect("Missing eBPF map");
    }

    fn get_next_map_id(&self, start_id: BpfMapId) -> Option<BpfMapId> {
        self.maps
            .lock()
            .range((Bound::Excluded(start_id), Bound::Unbounded))
            .next()
            .map(|(k, _)| *k)
    }

    fn get_map_by_id(&self, id: BpfMapId) -> Option<BpfMapHandle> {
        self.maps.lock().get(&id).map(|entry| entry.weak_map.upgrade()).flatten()
    }

    /// Removed socket with the specified `cookie` from all `sk_storage` maps.
    // TODO(https://fxbug.dev/496639039): Move sk_storage cleanup to Netstack.
    pub fn remove_sk_storage_entries(&self, cookie: u64) {
        self.maps.lock().iter().for_each(|(_, entry)| {
            if entry.map_type == bpf_map_type_BPF_MAP_TYPE_SK_STORAGE
                && let Some(map) = entry.weak_map.upgrade()
            {
                let _ = map.delete(cookie.as_bytes());
            }
        });
    }

    /// Clones `sk_storage` map entries with `BPF_F_CLONE` from `parent_cookie` socket to
    /// `child_cookie` socket.
    pub fn clone_sk_storage_entries(&self, parent_cookie: u64, child_cookie: u64) {
        self.maps.lock().iter().for_each(|(_, entry)| {
            if entry.map_type == bpf_map_type_BPF_MAP_TYPE_SK_STORAGE
                && let Some(map) = entry.weak_map.upgrade()
                && map.schema.flags.contains(MapFlags::Clone)
            {
                if let Some(mut val) = map.load(parent_cookie.as_bytes()) {
                    let _ = map.update(child_cookie.as_bytes(), (&mut val[..]).into(), 0);
                }
            }
        });
    }
}
