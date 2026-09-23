// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl_fuchsia_memory_stacktrack_client as fstacktrack_client;
use fuchsia_async as fasync;
use stacktrack_vmo::threads_table_v1::StacktrackReader;
use std::collections::HashSet;
use std::sync::LazyLock;
use zx::{self as zx, Koid};

use crate::process::{Process, Snapshot};
use crate::utils::find_executable_regions;

pub static PAGE_SIZE: LazyLock<u64> = LazyLock::new(|| zx::system_get_page_size() as u64);

pub struct ProcessV1 {
    koid: Koid,
    name: String,
    process: zx::Process,
    threads_table_vmo: zx::Vmo,
}

impl ProcessV1 {
    pub fn new(
        process: zx::Process,
        threads_table_vmo: zx::Vmo,
    ) -> Result<ProcessV1, anyhow::Error> {
        let name = process.get_name()?.to_string();
        let koid = process.koid()?;

        Ok(ProcessV1 { name, koid, process, threads_table_vmo })
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
        fasync::OnSignals::new(&self.process, zx::Signals::TASK_TERMINATED).await?;
        Ok(())
    }

    fn get_stack_traces(&self) -> Result<Box<dyn Snapshot>, anyhow::Error> {
        // Create a snapshot of the VMO, to freeze a coherent view of the
        // linked list at the current point in time.
        let size = self.threads_table_vmo.get_size()?;
        let snapshot = self.threads_table_vmo.create_child(
            zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE,
            0,
            size,
        )?;

        let executable_regions = find_executable_regions(&self.process)?;
        Ok(Box::new(SnapshotV1 { threads_table_vmo: snapshot, executable_regions }))
    }
}

pub struct SnapshotV1 {
    threads_table_vmo: zx::Vmo,
    executable_regions: Vec<fstacktrack_client::ExecutableRegion>,
}

#[async_trait]
impl Snapshot for SnapshotV1 {
    async fn write_to(
        &self,
        receiver: &mut fstacktrack_client::SnapshotReceiverProxy,
    ) -> Result<(), anyhow::Error> {
        // SAFETY: The snapshot VMO is immutable.
        let reader = unsafe { StacktrackReader::new(&self.threads_table_vmo)? };

        let mut streamer = stacktrack_snapshot::Streamer::new(receiver);

        streamer = streamer
            .push_element(fstacktrack_client::SnapshotElement::PageSize(*PAGE_SIZE))
            .await?;

        for region in &self.executable_regions {
            streamer = streamer
                .push_element(fstacktrack_client::SnapshotElement::ExecutableRegion(region.clone()))
                .await?;
        }

        // Iterate over the stack traces in the snapshot. If the same thread appears twice, only
        // consider its first (newer) entry.
        let mut seen_koids = HashSet::new();
        for node in reader.iter() {
            if !seen_koids.insert(node.koid) {
                continue;
            }

            let frames = node.frames[..node.count as usize]
                .iter()
                .map(|frame| fstacktrack_client::CallFrame {
                    program_address: frame.pc,
                    frame_pointer: frame.fp,
                })
                .collect();

            streamer = streamer
                .push_element(fstacktrack_client::SnapshotElement::StackTrace(
                    fstacktrack_client::StackTrace {
                        thread_koid: Some(node.koid),
                        frames: Some(frames),
                        ..Default::default()
                    },
                ))
                .await?;
        }

        streamer.end_of_snapshot().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_memory_stacktrack_process as fstacktrack_process;
    use futures::pin_mut;
    use itertools::{Itertools, assert_equal};
    use stacktrack_vmo::threads_table_v1::StacktrackWriter;
    use zx::Task;

    use crate::registry::Registry;

    /// Creates an empty threads table VMO and builds a Registry channel that ties it to the given
    /// process.
    fn setup_fake_process(
        process: &zx::Process,
    ) -> (fstacktrack_process::RegistryRequestStream, Koid, StacktrackWriter) {
        // Create and initialize the VMOs.
        const VMO_SIZE: u64 = 1 << 20; // 1MB
        let threads_table_vmo = zx::Vmo::create(VMO_SIZE).unwrap();

        // SAFETY: Nobody else will directly access the threads_table_vmo. Readers will always take
        // a snapshot first and read that instead.
        let threads_table_writer = unsafe { StacktrackWriter::new(&threads_table_vmo) }.unwrap();

        let koid = process.koid().unwrap();

        // Create channels and send the registration message.
        let (registry_proxy, registry_stream) =
            create_proxy_and_stream::<fstacktrack_process::RegistryMarker>();
        registry_proxy
            .register_v1(
                process.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                threads_table_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            )
            .unwrap();

        (registry_stream, koid, threads_table_writer)
    }

    fn setup_fake_process_from_self()
    -> (fstacktrack_process::RegistryRequestStream, Koid, StacktrackWriter) {
        setup_fake_process(&fuchsia_runtime::process_self())
    }

    // Asserts that the given list of executable regions correctly describes the self process.
    fn assert_executable_regions_valid_for_process_self(
        actual: &std::collections::HashMap<u64, stacktrack_snapshot::ExecutableRegion>,
    ) {
        // Enumerate the expected executable regions.
        let expected = find_executable_regions(&fuchsia_runtime::process_self())
            .unwrap()
            .into_iter()
            .map(|region| {
                (
                    region.address.unwrap(),
                    region.size.unwrap(),
                    region.vaddr.unwrap(),
                    region.build_id.unwrap().value,
                )
            })
            .sorted_by_key(|(addr, _, _, _)| *addr);

        // Convert the actual regions to the same format, so that they can be compared.
        let actual = actual
            .iter()
            .map(|(address, region)| (*address, region.size, region.vaddr, region.build_id.clone()))
            .sorted_by_key(|(addr, _, _, _)| *addr);

        // Assert that both iterators return the same elements.
        assert_equal(actual, expected);
    }

    #[test]
    fn test_register_and_unregister() {
        let mut ex = fasync::TestExecutor::new();
        let registry = Registry::new();

        // Create a child process that we can kill. We don't need to start it, just having a handle
        // to it is enough for ProcessV1 to wait on it.
        let (process, _root_vmar) = zx::Process::create(
            &fuchsia_runtime::job_default(),
            zx::Name::new("fake-process").unwrap(),
            zx::ProcessOptions::empty(),
        )
        .unwrap();

        // Setup fake process.
        let (registry_stream, koid, _) = setup_fake_process(&process);
        let name = "fake-process".to_string();

        // Register it.
        let serve_fut = registry.serve_process_stream(registry_stream);
        pin_mut!(serve_fut);
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Verify that the registry now contains the process.
        assert_eq!(registry.list_processes(), [(koid, name)]);

        // Kill the process to simulate exit.
        process.kill().unwrap();

        // Verify that the registry no longer contains the process.
        assert!(ex.run_until_stalled(&mut serve_fut).is_ready());
        assert_eq!(registry.list_processes(), []);
    }

    #[test]
    fn test_get_stack_traces() {
        let mut ex = fasync::TestExecutor::new();
        let registry = std::rc::Rc::new(Registry::new());

        // Setup fake process.
        let (registry_stream, koid, mut writer) = setup_fake_process_from_self();

        // Register it.
        let serve_fut = registry.serve_process_stream(registry_stream);
        pin_mut!(serve_fut);
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Fill the VMO with some data.
        let frames = [
            stacktrack_vmo::threads_table_v1::Frame { pc: 0x1111, fp: 0x2222 },
            stacktrack_vmo::threads_table_v1::Frame { pc: 0x3333, fp: 0x4444 },
        ];
        writer.insert_at_head(koid.raw_koid(), &frames).unwrap();

        // Get stack traces.
        let received_snapshot = ex.run_singlethreaded(async {
            let (mut receiver_proxy, receiver_stream) =
                create_proxy_and_stream::<fstacktrack_client::SnapshotReceiverMarker>();
            let receive_worker =
                fasync::Task::local(stacktrack_snapshot::Snapshot::receive_from(receiver_stream));

            let process = registry.get_process(&koid).unwrap();
            let snapshot = process.get_stack_traces().unwrap();
            snapshot.write_to(&mut receiver_proxy).await.expect("failed to write snapshot");

            receive_worker.await.expect("failed to receive snapshot")
        });

        // Verify executable regions.
        assert_executable_regions_valid_for_process_self(&received_snapshot.executable_regions);

        // Verify stack traces.
        assert_eq!(received_snapshot.stack_traces.len(), 1);
        let stack_trace = received_snapshot.stack_traces.get(&koid.raw_koid()).unwrap();
        assert_eq!(stack_trace.frames.len(), 2);
        assert_eq!(stack_trace.frames[0].program_address, 0x1111);
        assert_eq!(stack_trace.frames[0].frame_pointer, 0x2222);
        assert_eq!(stack_trace.frames[1].program_address, 0x3333);
        assert_eq!(stack_trace.frames[1].frame_pointer, 0x4444);
    }
}
