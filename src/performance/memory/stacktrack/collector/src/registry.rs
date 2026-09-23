// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl_fuchsia_memory_stacktrack_client::{
    self as fstacktrack_client, CollectorError, ProcessSelector,
};
use fidl_fuchsia_memory_stacktrack_process as fstacktrack_process;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use log::{info, warn};
use std::collections::hash_map::{Entry, HashMap};
use std::sync::Arc;
use zx::{self as zx, Koid};

use crate::process::Process;
use crate::process_v1::ProcessV1;

pub struct Registry {
    processes: Mutex<HashMap<Koid, Arc<dyn Process>>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { processes: Mutex::new(HashMap::new()) }
    }

    fn find_process(
        &self,
        selector: &Option<ProcessSelector>,
    ) -> Result<Arc<dyn Process>, CollectorError> {
        let mut matches: Vec<_> = {
            let processes = self.processes.lock();
            match selector {
                None => processes.values().cloned().collect(),
                Some(ProcessSelector::ByName(name)) => {
                    processes.values().filter(|p| p.get_name() == name).cloned().collect()
                }
                Some(ProcessSelector::ByKoid(koid)) => {
                    processes.get(&Koid::from_raw(*koid)).into_iter().cloned().collect()
                }
                Some(selector @ fstacktrack_client::ProcessSelectorUnknown!()) => {
                    warn!("Unknown process selector: {:?}", selector);
                    return Err(CollectorError::ProcessSelectorUnsupported);
                }
            }
        };

        if matches.len() > 1 {
            // More than one match.
            Err(CollectorError::ProcessSelectorAmbiguous)
        } else if let Some(process) = matches.pop() {
            // Exactly one match.
            Ok(process)
        } else {
            Err(CollectorError::ProcessSelectorNoMatch)
        }
    }

    pub async fn serve_client_stream(
        &self,
        mut stream: fstacktrack_client::CollectorRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(request) = stream.next().await.transpose()? {
            match request {
                fstacktrack_client::CollectorRequest::GetStackTraces { payload, .. } => {
                    let mut receiver =
                        payload.receiver.context("missing required receiver")?.into_proxy();
                    let process_selector = payload.process_selector;

                    let process = self.find_process(&process_selector);

                    start_detached_task(async move {
                        let error = match process {
                            Ok(process) => match process.get_stack_traces() {
                                Ok(snapshot) => match snapshot.write_to(&mut receiver).await {
                                    Ok(()) => return Ok(()),
                                    Err(error) => {
                                        warn!(error:?; "Failed to write snapshot");
                                        CollectorError::GetStackTracesFailed
                                    }
                                },
                                Err(error) => {
                                    warn!(error:?; "Failed to get stack traces");
                                    CollectorError::GetStackTracesFailed
                                }
                            },
                            Err(error) => {
                                warn!(error:?; "Failed to find process");
                                error
                            }
                        };
                        receiver.report_error(error).await.context("reporting error")
                    });
                }
                fstacktrack_client::CollectorRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal; "Unknown CollectorRequest");
                }
            }
        }
        Ok(())
    }

    pub async fn serve_process_stream(
        &self,
        mut stream: fstacktrack_process::RegistryRequestStream,
    ) -> Result<(), anyhow::Error> {
        let registration_request = stream
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("No registration message was received"))??;

        let process: Arc<dyn Process> = match registration_request {
            fstacktrack_process::RegistryRequest::RegisterV1 {
                process, threads_table_vmo, ..
            } => {
                let process = ProcessV1::new(process, threads_table_vmo)?;
                Arc::new(process)
            }
            fstacktrack_process::RegistryRequest::_UnknownMethod { ordinal, .. } => {
                anyhow::bail!("Unknown RegistryRequest (ordinal={})", ordinal);
            }
        };

        self.serve_process(process).await
    }

    #[cfg(test)]
    pub fn get_process(&self, koid: &Koid) -> Option<Arc<dyn Process>> {
        self.processes.lock().get(koid).cloned()
    }

    #[cfg(test)]
    pub fn list_processes(&self) -> Vec<(Koid, String)> {
        let mut processes: Vec<_> = self
            .processes
            .lock()
            .iter()
            .map(|(koid, process)| (*koid, process.get_name().to_string()))
            .collect();
        processes.sort_by_key(|(koid, _)| *koid);
        processes
    }

    async fn serve_process(&self, process: Arc<dyn Process>) -> Result<(), anyhow::Error> {
        let process_koid = process.get_koid();

        info!(koid = process_koid.raw_koid(), name = process.get_name(); "Process connected");
        match self.processes.lock().entry(process_koid) {
            Entry::Vacant(vacant_entry) => vacant_entry.insert(Arc::clone(&process)),
            Entry::Occupied(_) => {
                anyhow::bail!("Another process with the same koid is already connected")
            }
        };

        let status = process.serve_until_exit().await;

        info!(koid = process_koid.raw_koid(), name = process.get_name(); "Process disconnected");
        self.processes.lock().remove(&process_koid).expect("Koid should still be present");

        // Propagate error only after removing the entry from `processes`.
        status
    }
}

fn start_detached_task(fut: impl futures::Future<Output = anyhow::Result<()>> + 'static) {
    let worker_fn = async move {
        if let Err(error) = fut.await {
            warn!(error:?; "Error in detached task");
        }
    };
    fasync::Task::local(worker_fn).detach();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::{create_proxy_and_stream, create_request_stream};
    use futures::channel::oneshot;
    use futures::pin_mut;
    use test_case::test_case;

    use crate::process::Snapshot;

    fn create_registry_and_proxy(
        initial_processes: impl IntoIterator<Item = Arc<dyn Process>>,
    ) -> (Arc<Registry>, fstacktrack_client::CollectorProxy) {
        // Create a new Registry and register the given processes.
        let registry = Arc::new(Registry::new());
        {
            let mut processes = registry.processes.lock();
            for process in initial_processes {
                let koid = process.get_koid();
                let insert_result = processes.insert(koid, process);
                assert!(insert_result.is_none(), "found duplicate koid in initial_processes");
            }
        }

        // Create a client and start serving its stream from a detached task.
        let proxy = {
            let registry = registry.clone();
            let (proxy, stream) = create_proxy_and_stream::<fstacktrack_client::CollectorMarker>();

            let worker_fn = async move { registry.serve_client_stream(stream).await.unwrap() };
            fasync::Task::local(worker_fn).detach();

            proxy
        };

        (registry, proxy)
    }

    struct FakeProcess {
        name: String,
        koid: Koid,
        exit_signal: Mutex<Option<oneshot::Receiver<Result<(), anyhow::Error>>>>,
        get_stack_traces_succeeds: bool,
    }

    impl FakeProcess {
        /// Returns a new FakeProcess and a oneshot channel to make it exit with a given result.
        pub fn new(
            name: &str,
            koid: Koid,
            get_stack_traces_succeeds: bool,
        ) -> (Arc<dyn Process>, oneshot::Sender<anyhow::Result<()>>) {
            let (sender, receiver) = oneshot::channel();
            let fake_process = FakeProcess {
                name: name.to_string(),
                koid,
                exit_signal: Mutex::new(Some(receiver)),
                get_stack_traces_succeeds,
            };
            (Arc::new(fake_process), sender)
        }
    }

    #[async_trait]
    impl Process for FakeProcess {
        fn get_name(&self) -> &str {
            &self.name
        }

        fn get_koid(&self) -> Koid {
            self.koid
        }

        async fn serve_until_exit(&self) -> Result<(), anyhow::Error> {
            let exit_signal = self.exit_signal.lock().take().unwrap();
            exit_signal.await?
        }

        fn get_stack_traces(&self) -> Result<Box<dyn Snapshot>, anyhow::Error> {
            if self.get_stack_traces_succeeds {
                Ok(Box::new(FakeSnapshot {}))
            } else {
                Err(anyhow::anyhow!("Get stack traces failed"))
            }
        }
    }

    struct FakeSnapshot {}

    const FAKE_PAGE_SIZE: u64 = 4096;
    const FAKE_THREAD_KOID: u64 = 1111;
    const FAKE_FRAME_PC: u64 = 0x1234;
    const FAKE_FRAME_FP: u64 = 0x5678;

    #[async_trait]
    impl Snapshot for FakeSnapshot {
        async fn write_to(
            &self,
            dest: &mut fstacktrack_client::SnapshotReceiverProxy,
        ) -> Result<(), anyhow::Error> {
            let fut = dest.batch(&[fstacktrack_client::SnapshotElement::PageSize(FAKE_PAGE_SIZE)]);
            fut.await?;

            let fut = dest.batch(&[fstacktrack_client::SnapshotElement::StackTrace(
                fstacktrack_client::StackTrace {
                    thread_koid: Some(FAKE_THREAD_KOID),
                    frames: Some(vec![fstacktrack_client::CallFrame {
                        program_address: FAKE_FRAME_PC,
                        frame_pointer: FAKE_FRAME_FP,
                    }]),
                    ..Default::default()
                },
            )]);
            fut.await?;

            let fut = dest.batch(&[]);
            fut.await?;

            Ok(())
        }
    }

    impl FakeSnapshot {
        /// Receives a Snapshot from a SnapshotReceiver channel and asserts that it matches the
        /// output of `write_to`.
        async fn receive_and_assert_match(src: fstacktrack_client::SnapshotReceiverRequestStream) {
            let received_snapshot = stacktrack_snapshot::Snapshot::receive_from(src).await.unwrap();
            assert_eq!(received_snapshot.page_size, FAKE_PAGE_SIZE);
            assert_eq!(received_snapshot.stack_traces.len(), 1);
            let stack_trace = received_snapshot.stack_traces.get(&FAKE_THREAD_KOID).unwrap();
            assert_eq!(stack_trace.frames.len(), 1);
            assert_eq!(stack_trace.frames[0].program_address, FAKE_FRAME_PC);
            assert_eq!(stack_trace.frames[0].frame_pointer, FAKE_FRAME_FP);
        }
    }

    async fn receive_and_assert_error(
        mut src: fstacktrack_client::SnapshotReceiverRequestStream,
        expected_error: fstacktrack_client::CollectorError,
    ) {
        let received = src.next().await;
        assert_matches!(
            received,
            Some(Ok(fstacktrack_client::SnapshotReceiverRequest::ReportError{ error, .. })) if error == expected_error
        );
    }

    #[test_case(Ok(()) ; "exit ok")]
    #[test_case(Err(anyhow::anyhow!("Simulated error")) ; "exit error")]
    fn test_register_and_unregister(exit_result: Result<(), anyhow::Error>) {
        let mut ex = fasync::TestExecutor::new();
        let registry = Registry::new();

        // Setup fake process and register it.
        let name = "fake";
        let koid = Koid::from_raw(1234);
        let (process, signal) = FakeProcess::new(name, koid, false);
        let serve_fut = registry.serve_process(process);
        pin_mut!(serve_fut);
        assert!(ex.run_until_stalled(&mut serve_fut).is_pending());

        // Verify that the registry now contains the process.
        assert_eq!(registry.list_processes(), [(koid, name.to_string())]);

        // Simulate process exit.
        signal.send(exit_result).unwrap();
        assert!(ex.run_until_stalled(&mut serve_fut).is_ready());

        // Verify that the registry no longer contains the process.
        assert_eq!(registry.list_processes(), []);
    }

    #[test]
    fn test_cannot_register_same_koid_twice() {
        let mut ex = fasync::TestExecutor::new();
        let registry = Registry::new();

        // Create two FakeProcess instances with the same koid.
        let name1 = "fake-1";
        let name2 = "fake-2";
        let koid = Koid::from_raw(1234);
        let (process1, _signal1) = FakeProcess::new(name1, koid, false);
        let (process2, _signal2) = FakeProcess::new(name2, koid, false);

        // Register the first process.
        let serve1_fut = registry.serve_process(process1);
        pin_mut!(serve1_fut);
        assert!(ex.run_until_stalled(&mut serve1_fut).is_pending());

        // Verify that the registry now contains the process.
        assert_eq!(registry.list_processes(), [(koid, name1.to_string())]);

        // Verify that the second process cannot be registered (serve_process should exit
        // immediately).
        let serve2_fut = registry.serve_process(process2);
        pin_mut!(serve2_fut);
        assert!(ex.run_until_stalled(&mut serve2_fut).is_ready());
        assert_eq!(registry.list_processes(), [(koid, name1.to_string())]);

        // Verify that the first process stayed registered as if nothing happened.
        assert!(ex.run_until_stalled(&mut serve1_fut).is_pending());
    }

    #[test_case(Some(fstacktrack_client::ProcessSelector::ByKoid(3)),
        None ; "valid koid, snapshot succeeds")]
    #[test_case(Some(fstacktrack_client::ProcessSelector::ByName("foo".to_string())),
        Some(CollectorError::GetStackTracesFailed) ; "valid name, snapshot fails")]
    #[test_case(Some(fstacktrack_client::ProcessSelector::ByName("bar".to_string())),
        Some(CollectorError::ProcessSelectorAmbiguous) ; "ambiguous name")]
    #[test_case(Some(fstacktrack_client::ProcessSelector::ByName("baz".to_string())),
        Some(CollectorError::ProcessSelectorNoMatch) ; "no matching name")]
    #[test_case(Some(fstacktrack_client::ProcessSelector::ByKoid(2)),
        Some(CollectorError::GetStackTracesFailed) ; "valid koid, snapshot fails")]
    #[test_case(Some(fstacktrack_client::ProcessSelector::ByKoid(99)),
        Some(CollectorError::ProcessSelectorNoMatch) ; "no matching koid")]
    #[test_case(None,
        Some(CollectorError::ProcessSelectorAmbiguous) ; "missing process selector")]
    #[fuchsia::test]
    async fn test_get_stack_traces(
        process_selector: Option<fstacktrack_client::ProcessSelector>,
        expect_error: Option<CollectorError>,
    ) {
        // Create three FakeProcess instances, two of which with the same name.
        // The first two processes return a LiveSnapshotFailed error; the third one successfully
        // returns a snapshot.
        let (process1, _signal1) = FakeProcess::new("foo", Koid::from_raw(1), false);
        let (process2, _signal2) = FakeProcess::new("bar", Koid::from_raw(2), false);
        let (process3, _signal3) = FakeProcess::new("bar", Koid::from_raw(3), true);

        // Create a Registry and a client connected to it.
        let (_registry, proxy) = create_registry_and_proxy([process1, process2, process3]);

        // Execute the request.
        let (receiver_client, receiver_stream) = create_request_stream();
        let request = fstacktrack_client::CollectorGetStackTracesRequest {
            process_selector,
            receiver: Some(receiver_client),
            ..Default::default()
        };
        proxy.get_stack_traces(request).expect("FIDL channel error");

        // Verify that the result matches our expectation (either success or a specific error).
        if let Some(expect_error) = expect_error {
            receive_and_assert_error(receiver_stream, expect_error).await;
        } else {
            FakeSnapshot::receive_and_assert_match(receiver_stream).await;
        }
    }
}
