// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_memory_heapdump_client as fheapdump_client;
use fidl_fuchsia_memory_heapdump_process as fheapdump_process;
use futures::StreamExt;
use futures::lock::Mutex;
use heapdump_vmo::allocations_table_v1::AllocationsTableReader;
use heapdump_vmo::resources_table_v1::ResourcesTableReader;
use heapdump_vmo::stack_trace_compression;
use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use zx::Koid;

use crate::process::{Process, Snapshot};
use crate::snapshot_storage::SnapshotStorage;
use crate::utils::{find_executable_regions, read_process_memory};

fn get_process_name(process: &zx::Process) -> Result<String, anyhow::Error> {
    Ok(process.get_name()?.to_string())
}

/// An instrumented process that speaks version 1 of the protocol.
///
/// In particular, version 1 of the protocol implies the usage of:
/// - `allocations_table_v1` for the `allocations_vmo`
/// - `resources_table_v1` for the `resources_vmo`
pub struct ProcessV1 {
    koid: Koid,
    name: String,
    process: zx::Process,
    allocations_vmo: zx::Vmo,
    resources_vmo: Arc<zx::Vmo>,
    snapshot_stream: Mutex<fheapdump_process::SnapshotSinkV1RequestStream>,
    snapshot_storage: Arc<Mutex<SnapshotStorage>>,
}

impl ProcessV1 {
    pub fn new(
        process: zx::Process,
        allocations_vmo: zx::Vmo,
        resources_vmo: zx::Vmo,
        snapshot_sink: ServerEnd<fheapdump_process::SnapshotSinkV1Marker>,
        snapshot_storage: Arc<Mutex<SnapshotStorage>>,
    ) -> Result<ProcessV1, anyhow::Error> {
        let name = get_process_name(&process)?;
        let koid = process.koid()?;
        Ok(ProcessV1 {
            name,
            koid,
            process,
            allocations_vmo,
            resources_vmo: Arc::new(resources_vmo),
            snapshot_stream: Mutex::new(snapshot_sink.into_stream()),
            snapshot_storage,
        })
    }

    fn finalize_snapshot(
        &self,
        allocations_vmo_snapshot: zx::Vmo,
        block_contents: HashMap<u64, Vec<u8>>,
    ) -> Result<Box<dyn Snapshot>, zx::Status> {
        let executable_regions = find_executable_regions(&self.process)?;
        Ok(Box::new(SnapshotV1::new(
            allocations_vmo_snapshot,
            self.resources_vmo.clone(),
            executable_regions,
            block_contents,
        )))
    }
}

#[async_trait]
impl Process for ProcessV1 {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_koid(&self) -> Koid {
        self.koid
    }

    async fn serve_until_exit(&self) -> Result<(), anyhow::Error> {
        let mut stream = self.snapshot_stream.lock().await;
        while let Some(request) = stream.next().await.transpose()? {
            match request {
                fheapdump_process::SnapshotSinkV1Request::StoreNamedSnapshot {
                    snapshot_name,
                    allocations_vmo_snapshot,
                    ..
                } => match self.finalize_snapshot(allocations_vmo_snapshot, HashMap::new()) {
                    Ok(snapshot) => {
                        let mut snapshot_storage = self.snapshot_storage.lock().await;
                        let snapshot_id = snapshot_storage.add_snapshot(
                            self.name.clone(),
                            self.koid,
                            snapshot_name.clone(),
                            snapshot,
                        );
                        info!(
                            snapshot_id,
                            snapshot_name = snapshot_name.as_str(),
                            process_koid = self.koid.raw_koid(),
                            process_name = self.name.as_str();
                            "Stored snapshot"
                        );
                    }
                    Err(status) => {
                        warn!(
                            koid = self.koid.raw_koid(),
                            name = self.name.as_str(),
                            status:?;
                            "Failed to process snapshot"
                        );
                    }
                },
            }
        }

        Ok(())
    }

    fn take_live_snapshot(&self, with_contents: bool) -> Result<Box<dyn Snapshot>, anyhow::Error> {
        let size = self.allocations_vmo.get_size()?;
        let snapshot = self.allocations_vmo.create_child(
            zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE,
            0,
            size,
        )?;

        let mut block_contents = HashMap::new();
        if with_contents {
            // SAFETY: The snapshot VMO is immutable. Besides, nobody else has access to it.
            let allocations_table = unsafe { AllocationsTableReader::new(&snapshot) }?;
            for block in allocations_table.iter() {
                let block = block?;
                match read_process_memory(&self.process, block.address, block.size) {
                    Ok(data) => {
                        block_contents.insert(block.address, data);
                    }
                    Err(error) => {
                        // Do not fail the whole snapshot if reading a block fails. Some failures
                        // are unavoidable because we do not freeze the process while reading, and
                        // it can unmap its memory in the meantime.
                        warn!(
                            error:?,
                            address = format!("{:#x}", block.address).as_str(),
                            size = block.size;
                            "Error while reading process memory"
                        );
                    }
                }
            }
        }

        Ok(self.finalize_snapshot(snapshot, block_contents)?)
    }
}

/// A snapshot of a V1 process.
pub struct SnapshotV1 {
    allocations_vmo: zx::Vmo,
    resources_vmo: Arc<zx::Vmo>,
    executable_regions: Vec<fheapdump_client::ExecutableRegion>,
    block_contents: HashMap<u64, Vec<u8>>,
}

impl SnapshotV1 {
    pub fn new(
        allocations_vmo: zx::Vmo,
        resources_vmo: Arc<zx::Vmo>,
        executable_regions: Vec<fheapdump_client::ExecutableRegion>,
        block_contents: HashMap<u64, Vec<u8>>,
    ) -> SnapshotV1 {
        SnapshotV1 { allocations_vmo, resources_vmo, executable_regions, block_contents }
    }
}

#[async_trait]
impl Snapshot for SnapshotV1 {
    async fn write_to(
        &self,
        dest: &mut fheapdump_client::SnapshotReceiverProxy,
    ) -> Result<(), anyhow::Error> {
        let mut streamer = heapdump_snapshot::Streamer::new(dest);

        // SAFETY: The allocations_vmo is immutable.
        let allocations_table = unsafe { AllocationsTableReader::new(&self.allocations_vmo) }?;

        // SAFETY: We will only access resources_vmo through ResourceKeys created by the
        // corresponding AllocationsTableWriter running inside the instrumented process.
        let resources_table = unsafe { ResourcesTableReader::new(&self.resources_vmo) }?;

        let mut thread_info_keys = HashSet::new();
        let mut stack_trace_keys = HashSet::new();
        for block in allocations_table.iter() {
            let block = block?;
            thread_info_keys.insert(block.thread_info_key);
            stack_trace_keys.insert(block.stack_trace_key);

            streamer = streamer
                .push_element(fheapdump_client::SnapshotElement::Allocation(
                    fheapdump_client::Allocation {
                        address: Some(block.address),
                        size: Some(block.size),
                        thread_info_key: Some(block.thread_info_key.into_raw() as u64),
                        stack_trace_key: Some(block.stack_trace_key.into_raw() as u64),
                        timestamp: Some(block.timestamp),
                        ..Default::default()
                    },
                ))
                .await?;
        }

        for thread_info_key in thread_info_keys {
            let thread_info = resources_table.get_thread_info(thread_info_key)?;

            streamer = streamer
                .push_element(fheapdump_client::SnapshotElement::ThreadInfo(
                    fheapdump_client::ThreadInfo {
                        thread_info_key: Some(thread_info_key.into_raw() as u64),
                        koid: Some(thread_info.koid),
                        name: Some(thread_info.name.to_string()),
                        ..Default::default()
                    },
                ))
                .await?;
        }

        for stack_trace_key in stack_trace_keys {
            let compressed_stack_trace =
                resources_table.get_compressed_stack_trace(stack_trace_key)?;
            let uncompressed_stack_trace =
                stack_trace_compression::uncompress(compressed_stack_trace)?;

            // Split long stack traces in chunks so that they can be paginated.
            const MAX_ENTRIES_PER_CHUNK: usize = 32;
            for chunk in uncompressed_stack_trace.chunks(MAX_ENTRIES_PER_CHUNK) {
                streamer = streamer
                    .push_element(fheapdump_client::SnapshotElement::StackTrace(
                        fheapdump_client::StackTrace {
                            stack_trace_key: Some(stack_trace_key.into_raw() as u64),
                            program_addresses: Some(
                                chunk.iter().map(|addr| *addr as u64).collect(),
                            ),
                            ..Default::default()
                        },
                    ))
                    .await?;
            }
        }

        for executable_region in &self.executable_regions {
            streamer = streamer
                .push_element(fheapdump_client::SnapshotElement::ExecutableRegion(
                    executable_region.clone(),
                ))
                .await?;
        }

        for (block_address, block_data) in &self.block_contents {
            // Split long payloads in chunks so that they can be paginated.
            const MAX_DATA_PER_CHUNK: usize = 1024;
            for chunk in block_data.chunks(MAX_DATA_PER_CHUNK) {
                streamer = streamer
                    .push_element(fheapdump_client::SnapshotElement::BlockContents(
                        fheapdump_client::BlockContents {
                            address: Some(*block_address),
                            contents: Some(chunk.to_vec()),
                            ..Default::default()
                        },
                    ))
                    .await?;
            }
        }

        Ok(streamer.end_of_snapshot().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use fuchsia_async as fasync;
    use futures::pin_mut;
    use heapdump_snapshot::ThreadInfo;
    use heapdump_vmo::allocations_table_v1::AllocationsTableWriter;
    use heapdump_vmo::resources_table_v1::ResourcesTableWriter;
    use itertools::{Itertools, assert_equal};
    use std::pin::Pin;
    use std::rc::Rc;
    use test_case::test_case;

    use crate::registry::Registry;

    /// Creates empty allocation and resource VMOs and builds a Registry channel that ties them to
    /// the current process.
    ///
    /// By reusing the current process, instead of creating a new one, we avoid requiring the
    /// ZX_POL_NEW_PROCESS capability and we can easily control the contents of the "fake" process'
    /// memory.
    ///
    /// The returned tuple consists of:
    /// - the server end of the channel, to be connected a Registry instance
    /// - the koid of the fake (== current) process
    /// - two writers, that can used to manipulate the corresponding VMOs
    /// - a function that, given a string, sends a named snapshot of the current contents of the
    ///   allocations VMO over the SnapshotSink channel
    fn setup_fake_process_from_self() -> (
        fheapdump_process::RegistryRequestStream,
        Koid,
        AllocationsTableWriter,
        ResourcesTableWriter,
        impl Fn(&str),
    ) {
        // Create and initialize the VMOs.
        const VMO_SIZE: u64 = 1 << 31;
        let allocations_vmo = zx::Vmo::create(VMO_SIZE).unwrap();
        let resources_vmo = zx::Vmo::create(VMO_SIZE).unwrap();

        // SAFETY: Nobody else will directly access the allocations_vmo. Readers will always take a
        // snapshot first and read that instead.
        let allocations_writer = unsafe { AllocationsTableWriter::new(&allocations_vmo) }.unwrap();

        // SAFETY: All readers of this VMO will synchronize by means of the ResourceKeys.
        let resources_writer = unsafe { ResourcesTableWriter::new(&resources_vmo) }.unwrap();

        let process =
            fuchsia_runtime::process_self().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let koid = process.koid().unwrap();

        // Create channels and send the registration message.
        let (registry_proxy, registry_stream) =
            create_proxy_and_stream::<fheapdump_process::RegistryMarker>();
        let (snapshot_proxy, snapshot_server) =
            create_proxy::<fheapdump_process::SnapshotSinkV1Marker>();
        registry_proxy
            .register_v1(
                process,
                allocations_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                resources_vmo,
                snapshot_server,
            )
            .unwrap();

        // Create a function that sends a named snapshot of the current allocation VMO contents.
        let snapshot_sink_fn = move |snapshot_name: &str| {
            let allocations_vmo_snapshot = allocations_vmo
                .create_child(
                    zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE,
                    0,
                    VMO_SIZE,
                )
                .unwrap();
            snapshot_proxy.store_named_snapshot(snapshot_name, allocations_vmo_snapshot).unwrap()
        };

        (registry_stream, koid, allocations_writer, resources_writer, snapshot_sink_fn)
    }

    // Asserts that the given list of executable regions correctly describes the fake process.
    //
    // Note: the fake process (created by `setup_fake_process_from_self`) is actually the current
    // process. Therefore, this function compares the regions described in `actual` to the list of
    // the executable regions in the current process.
    fn assert_executable_regions_valid_for_fake_process(
        actual: &HashMap<u64, heapdump_snapshot::ExecutableRegion>,
    ) {
        // Enumerate the expected executable regions.
        let expected = find_executable_regions(&fuchsia_runtime::process_self())
            .unwrap()
            .into_iter()
            .map(|region| {
                (
                    region.address.unwrap(),
                    region.size.unwrap(),
                    region.file_offset.unwrap(),
                    region.build_id.unwrap().value,
                )
            });

        // Convert the actual regions to the same format, so that they can be compared.
        let actual = actual.iter().map(|(address, region)| {
            (*address, region.size, region.file_offset, region.build_id.clone())
        });

        // Assert that both iterators return the same elements.
        assert_equal(actual.sorted(), expected.sorted());
    }

    #[test]
    fn test_register_and_unregister() {
        let mut ex = fasync::TestExecutor::new();
        let registry = Registry::new();

        // Setup fake process.
        let (registry_stream, koid, _, _, snapshot_sink_fn) = setup_fake_process_from_self();
        let name = get_process_name(&fuchsia_runtime::process_self()).unwrap();

        // Register it.
        let serve_fut = registry.serve_process_stream(registry_stream);
        pin_mut!(serve_fut);
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Verify that the registry now contains the process.
        assert_eq!(registry.list_processes(), [(koid, name)]);

        // Simulate process exit by dropping the snapshot sink channel.
        std::mem::drop(snapshot_sink_fn);
        assert!(ex.run_until_stalled(&mut serve_fut).is_ready());

        // Verify that the registry no longer contains the process.
        assert_eq!(registry.list_processes(), []);
    }

    const FAKE_THREAD_KOID: u64 = 1234;
    const FAKE_THREAD_NAME: zx::Name = zx::Name::new_lossy("fake-thread-name");

    // Generate a very long stack trace to verify that the pagination mechanism works.
    fn generate_fake_long_stack_trace() -> Vec<u64> {
        // Generate a stack trace such that its uncompressed size is four times bigger than
        // ZX_CHANNEL_MAX_MSG_BYTES.
        const UNCOMPRESSED_SIZE: usize = zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize * 4;
        const NUM_FRAMES: usize = UNCOMPRESSED_SIZE / std::mem::size_of::<u64>();
        (0..NUM_FRAMES as u64).collect()
    }

    // Generate a very long contents blob to verify that the pagination mechanism works.
    fn generate_fake_long_contents() -> Box<[u8]> {
        b"foobar".repeat(100000).into_boxed_slice()
    }

    #[test_case(false ; "without contents")]
    #[test_case(true ; "with contents")]
    fn test_take_live_snapshot(with_contents: bool) {
        let mut ex = fasync::TestExecutor::new();
        let registry = std::rc::Rc::new(Registry::new());

        // Setup fake process.
        let (
            registry_stream,
            koid,
            mut allocations_writer,
            mut resources_writer,
            _snapshot_sink_fn,
        ) = setup_fake_process_from_self();

        // Register it.
        let serve_fut = registry.serve_process_stream(registry_stream);
        pin_mut!(serve_fut);
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Record an allocation.
        let foobar = Pin::new(generate_fake_long_contents());
        let foobar_address = foobar.as_ptr() as u64;
        let foobar_size = foobar.len() as u64;
        let foobar_stack_trace = generate_fake_long_stack_trace();
        let foobar_timestamp = zx::MonotonicInstant::from_nanos(123456789);
        let foobar_thread_info_key =
            resources_writer.insert_thread_info(FAKE_THREAD_KOID, &FAKE_THREAD_NAME).unwrap();
        let (foobar_stack_trace_key, _) = resources_writer
            .intern_compressed_stack_trace(&stack_trace_compression::compress(
                &foobar_stack_trace.iter().map(|addr| *addr as usize).collect_vec(),
            ))
            .unwrap();
        allocations_writer
            .insert_allocation(
                foobar_address,
                foobar_size,
                foobar_thread_info_key,
                foobar_stack_trace_key,
                foobar_timestamp,
            )
            .unwrap();

        // Take a live snapshot.
        let mut received_snapshot = ex.run_singlethreaded(async {
            let (mut receiver_proxy, receiver_stream) =
                create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
            let receive_worker = fasync::Task::local(
                heapdump_snapshot::Snapshot::receive_single_from(receiver_stream),
            );

            let process = registry.get_process(&koid).unwrap();
            let snapshot = process.take_live_snapshot(with_contents).unwrap();
            snapshot.write_to(&mut receiver_proxy).await.expect("failed to write snapshot");

            receive_worker.await.expect("failed to receive snapshot")
        });

        // Verify that it contains our test allocation.
        let foobar_allocation = received_snapshot.allocations.swap_remove(
            received_snapshot
                .allocations
                .iter()
                .position(|alloc| alloc.address == Some(foobar_address))
                .unwrap(),
        );
        assert_eq!(foobar_allocation.size, foobar_size);
        assert_eq!(
            foobar_allocation.thread_info,
            Some(Rc::new(ThreadInfo {
                koid: FAKE_THREAD_KOID,
                name: FAKE_THREAD_NAME.to_string()
            }))
        );
        assert_eq!(foobar_allocation.stack_trace.program_addresses, foobar_stack_trace);
        assert_eq!(foobar_allocation.timestamp, Some(foobar_timestamp));
        if with_contents {
            // Verify that it has the expected contents.
            assert_eq!(
                foobar_allocation.contents.as_ref().expect("contents must be set"),
                &foobar.to_vec()
            );
        } else {
            // Verify that it has no contents.
            assert_matches!(foobar_allocation.contents, None);
        }
        assert!(received_snapshot.allocations.is_empty(), "all the entries have been removed");

        // Verify that it contains the expected executable regions.
        assert_executable_regions_valid_for_fake_process(&received_snapshot.executable_regions);
    }

    #[test]
    fn test_stored_snapshots() {
        let mut ex = fasync::TestExecutor::new();
        let registry = std::rc::Rc::new(Registry::new());

        // Setup fake process.
        let (
            registry_stream,
            _koid,
            mut allocations_writer,
            mut resources_writer,
            snapshot_sink_fn,
        ) = setup_fake_process_from_self();

        // Register it.
        let serve_fut = registry.serve_process_stream(registry_stream);
        pin_mut!(serve_fut);
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Send a stored snapshot called "snapshot-one" containing only a fake allocation.
        const FAKE_ALLOCATION_ADDRESS: u64 = 8888;
        const FAKE_ALLOCATION_SIZE: u64 = 120;
        const FAKE_ALLOCATION_STACK_TRACE: [u64; 2] = [12345, 67890];
        const FAKE_ALLOCATION_TIMESTAMP: zx::MonotonicInstant =
            zx::MonotonicInstant::from_nanos(1122334455);
        let thread_info_key =
            resources_writer.insert_thread_info(FAKE_THREAD_KOID, &FAKE_THREAD_NAME).unwrap();
        let (stack_trace_key, _) = resources_writer
            .intern_compressed_stack_trace(&stack_trace_compression::compress(
                &FAKE_ALLOCATION_STACK_TRACE.iter().map(|addr| *addr as usize).collect_vec(),
            ))
            .unwrap();
        allocations_writer
            .insert_allocation(
                FAKE_ALLOCATION_ADDRESS,
                FAKE_ALLOCATION_SIZE,
                thread_info_key,
                stack_trace_key,
                FAKE_ALLOCATION_TIMESTAMP,
            )
            .unwrap();
        snapshot_sink_fn("snapshot-one");

        // Remove the allocation and send a new stored snapshot called "snapshot-two".
        allocations_writer.erase_allocation(FAKE_ALLOCATION_ADDRESS).unwrap();
        snapshot_sink_fn("snapshot-two");

        // Let the registry process our requests to store the named snapshots.
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Verify that "snapshot-one" is present in the registry and that it only contains our
        // fake allocation.
        let mut receive_named_snapshot_fn = |snapshot_name| {
            ex.run_singlethreaded(async {
                let (mut receiver_proxy, receiver_stream) =
                    create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
                let receive_worker = fasync::Task::local(
                    heapdump_snapshot::Snapshot::receive_single_from(receiver_stream),
                );
                let snapshot = registry
                    .with_snapshot_storage(|snapshot_storage| {
                        snapshot_storage
                            .get_snapshot_by_name(snapshot_name)
                            .expect("snapshot not found")
                    })
                    .await;
                snapshot.write_to(&mut receiver_proxy).await.expect("failed to write snapshot");
                receive_worker.await.expect("failed to receive snapshot")
            })
        };
        let mut snapshot_one = receive_named_snapshot_fn("snapshot-one");
        let allocation = snapshot_one.allocations.swap_remove(
            snapshot_one
                .allocations
                .iter()
                .position(|alloc| alloc.address == Some(FAKE_ALLOCATION_ADDRESS))
                .unwrap(),
        );
        assert_eq!(allocation.size, FAKE_ALLOCATION_SIZE);
        assert_eq!(
            allocation.thread_info,
            Some(Rc::new(ThreadInfo {
                koid: FAKE_THREAD_KOID,
                name: FAKE_THREAD_NAME.to_string()
            }))
        );
        assert_eq!(allocation.stack_trace.program_addresses, FAKE_ALLOCATION_STACK_TRACE);
        assert_eq!(allocation.timestamp, Some(FAKE_ALLOCATION_TIMESTAMP));
        assert!(snapshot_one.allocations.is_empty(), "no other allocations should exist");
        assert_executable_regions_valid_for_fake_process(&snapshot_one.executable_regions);

        // Verify that "snapshot-two" is present too and that it does not contain any allocation.
        let snapshot_two = receive_named_snapshot_fn("snapshot-two");
        assert!(snapshot_two.allocations.is_empty(), "snapshot should be empty");
        assert_executable_regions_valid_for_fake_process(&snapshot_two.executable_regions);
    }
}
