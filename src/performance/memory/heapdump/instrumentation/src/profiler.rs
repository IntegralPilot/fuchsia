// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{ServerEnd, create_endpoints};
use fidl_fuchsia_memory_heapdump_process as fheapdump_process;
use std::sync::Mutex;
use std::time::Duration;

use crate::allocations_table::AllocationsTable;
use crate::resources_table::{ResourceKey, ResourcesTable};
use crate::waiter_list::WaiterList;
use crate::{
    heapdump_global_stats as HeapdumpGlobalStats,
    heapdump_thread_local_stats as HeapdumpThreadLocalStats,
};

/// A long timeout that will definitely never trigger if everything is working as intended.
///
/// Its only purpose if to prevent tests from hanging indefinitely if something is broken and a
/// waited address is never signaled.
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// The global instrumentation state for the current process (singleton).
///
/// This is the root of all the instrumentation's data structures except for per-thread data, which
/// is stored separately.
pub struct Profiler {
    snapshot_sink: fheapdump_process::SnapshotSinkV1SynchronousProxy,
    inner: Mutex<Option<ProfilerInner>>,
}

#[derive(Default)]
struct ProfilerInner {
    allocations_table: AllocationsTable,
    resources_table: ResourcesTable,
    global_stats: HeapdumpGlobalStats,
    snapshot_sink_server: Option<ServerEnd<fheapdump_process::SnapshotSinkV1Marker>>,
    waiters: WaiterList,
}

/// Per-thread instrumentation data.
#[derive(Default)]
pub struct PerThreadData {
    local_stats: HeapdumpThreadLocalStats,

    /// Koid of the current thread (cached).
    cached_koid: Option<zx::Koid>,

    /// The resource key of the current thread and its name at the time it was generated.
    resource_key_and_name: Option<(ResourceKey, zx::Name)>,
}

impl Default for Profiler {
    fn default() -> Profiler {
        let (client, server) = create_endpoints();
        let proxy = fheapdump_process::SnapshotSinkV1SynchronousProxy::new(client.into_channel());

        let inner = ProfilerInner { snapshot_sink_server: Some(server), ..Default::default() };
        Profiler { snapshot_sink: proxy, inner: Mutex::new(Some(inner)) }
    }
}

impl Profiler {
    pub fn bind(&self, registry_channel: zx::Channel) {
        let process_dup = fuchsia_runtime::process_self()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("failed to duplicate process handle");

        let (snapshot_sink_server, allocations_table_dup, resources_table_dup) = {
            let mut guard = self.inner.lock().unwrap();
            let inner = guard.as_mut().expect("profiler has been torn down");
            (
                inner.snapshot_sink_server.take(),
                inner.allocations_table.share_vmo(),
                inner.resources_table.share_vmo(),
            )
        };
        let snapshot_sink_server = snapshot_sink_server.expect("bind called more than once");

        let registry_proxy = fheapdump_process::RegistrySynchronousProxy::new(registry_channel);

        // Ignore result.
        let _ = registry_proxy.register_v1(
            process_dup,
            allocations_table_dup,
            resources_table_dup,
            snapshot_sink_server,
        );
    }

    pub fn teardown(&self) {
        self.inner.lock().unwrap().take().expect("profiler has already been torn down");
    }

    // Returns the global stats, or None if the Profiler has been torn down.
    pub fn get_global_stats(&self) -> Option<HeapdumpGlobalStats> {
        let guard = self.inner.lock().unwrap();
        match guard.as_ref() {
            Some(inner) => Some(inner.global_stats),
            None => None,
        }
    }

    fn intern(
        inner: &mut ProfilerInner,
        thread_data: &mut PerThreadData,
        compressed_stack_trace: &[u8],
    ) -> (ResourceKey, ResourceKey) {
        let (thread_koid, thread_name) =
            get_current_thread_koid_and_name(&mut thread_data.cached_koid);

        let thread_info_key = match thread_data.resource_key_and_name {
            Some((resource_key, old_name)) if old_name == thread_name => {
                // The previously generated resource key is still valid, because the name did not
                // change in the meantime.
                resource_key
            }
            _ => {
                // We need to generate a new resource key.
                let resource_key =
                    inner.resources_table.insert_thread_info(thread_koid, &thread_name);
                thread_data.resource_key_and_name = Some((resource_key, thread_name));
                resource_key
            }
        };
        let stack_trace_key = inner.resources_table.intern_stack_trace(compressed_stack_trace);

        (thread_info_key, stack_trace_key)
    }

    pub fn record_allocation(
        &self,
        thread_data: &mut PerThreadData,
        address: u64,
        size: u64,
        compressed_stack_trace: &[u8],
        timestamp: zx::MonotonicInstant,
    ) {
        let mut guard = self.inner.lock().unwrap();
        let Some(inner) = guard.as_mut() else {
            return;
        };

        let (thread_info_key, stack_trace_key) =
            Self::intern(inner, thread_data, compressed_stack_trace);

        // Insert the new entry. If a duplicate is found, it means that this allocation is recycling
        // a block that was just deallocated by realloc, but for which __scudo_realloc_allocate_hook
        // (i.e. the realloc end notification) has not been executed yet.
        loop {
            let Some(inner) = guard.as_mut() else {
                return; // The profiler has been torn down.
            };

            if inner.allocations_table.try_record_allocation(
                address,
                size,
                thread_info_key,
                stack_trace_key,
                timestamp,
            ) {
                // The allocation was successfully recorded. Let's update the stats too and we're
                // done.
                inner.global_stats.total_allocated_bytes += size;
                thread_data.local_stats.total_allocated_bytes += size;
                return;
            }

            guard = WaiterList::wait(guard, |inner| &mut inner.waiters, address, WAIT_TIMEOUT);
        }
    }

    pub fn forget_allocation(&self, thread_data: &mut PerThreadData, address: u64) {
        let mut guard = self.inner.lock().unwrap();
        let Some(inner) = guard.as_mut() else {
            return;
        };

        let size = inner.allocations_table.forget_allocation(address);

        inner.global_stats.total_deallocated_bytes += size;
        thread_data.local_stats.total_deallocated_bytes += size;

        // Notify the waiter (if any).
        inner.waiters.notify_one(address);
    }

    pub fn update_allocation(
        &self,
        thread_data: &mut PerThreadData,
        address: u64,
        size: u64,
        compressed_stack_trace: &[u8],
        timestamp: zx::MonotonicInstant,
    ) {
        let mut guard = self.inner.lock().unwrap();
        let Some(inner) = guard.as_mut() else {
            return;
        };

        let (thread_info_key, stack_trace_key) =
            Self::intern(inner, thread_data, compressed_stack_trace);

        let old_size = inner.allocations_table.update_allocation(
            address,
            size,
            thread_info_key,
            stack_trace_key,
            timestamp,
        );

        if size > old_size {
            let delta_allocated_bytes = size - old_size;
            inner.global_stats.total_allocated_bytes += delta_allocated_bytes;
            thread_data.local_stats.total_allocated_bytes += delta_allocated_bytes;
        } else {
            let delta_deallocated_bytes = old_size - size;
            inner.global_stats.total_deallocated_bytes += delta_deallocated_bytes;
            thread_data.local_stats.total_deallocated_bytes += delta_deallocated_bytes;
        }
    }

    pub fn publish_named_snapshot(&self, name: &str) {
        let allocations_table_snapshot = {
            let guard = self.inner.lock().unwrap();
            let Some(inner) = guard.as_ref() else {
                return;
            };
            inner.allocations_table.snapshot_vmo()
        };

        // Ignore outcome.
        let _ = self.snapshot_sink.store_named_snapshot(name, allocations_table_snapshot);
    }
}

impl PerThreadData {
    pub fn get_local_stats(&self) -> HeapdumpThreadLocalStats {
        self.local_stats
    }
}

fn get_current_thread_koid_and_name(koid_cache: &mut Option<zx::Koid>) -> (zx::Koid, zx::Name) {
    // Obtain the koid and the name of the current thread. Unlike the koid, the thread name
    // cannot be cached as it can change between calls.
    fuchsia_runtime::with_thread_self(|thread| {
        let koid = koid_cache
            .get_or_insert_with(|| thread.koid().expect("failed to get current thread's koid"));
        let name = thread.get_name().expect("failed to get current thread's name");
        (*koid, name)
    })
}
