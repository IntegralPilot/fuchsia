// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This library is only usable at API versions from which escrow is supported.

#[cfg(fuchsia_api_level_at_least = "HEAD")]
pub use everything::*;

#[cfg(fuchsia_api_level_less_than = "HEAD")]
mod everything {
    use anyhow as _;
    use fidl as _;
    use fidl_fuchsia_component_sandbox as _;
    use fidl_fuchsia_io as _;
    use fidl_fuchsia_process_lifecycle as _;
    use fuchsia_async as _;
    use fuchsia_component_runtime as _;
    use fuchsia_runtime as _;
    use fuchsia_sync as _;
    use futures as _;
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
mod everything {
    use anyhow::{Error, format_err};
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_component_sandbox as fsandbox;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_process_lifecycle as flifecycle;
    use fuchsia_async::Task;
    use fuchsia_component_runtime::Dictionary;
    use fuchsia_runtime::{self as fruntime, HandleInfo, HandleType};
    use fuchsia_sync::Mutex;
    use futures::StreamExt;
    use std::collections::HashSet;
    use std::process;
    use std::sync::Arc;

    /// Holds the process lifecycle channel (or optionally just the control side of it), and uses
    /// it to send escrow data and handles to component manager. This is a required step for a
    /// component to gracefully shut itself down when it would otherwise sit idle, in order to save
    /// system resources.
    #[derive(Default, Clone)]
    pub struct EscrowOperation {
        inner: Arc<Mutex<Inner>>,
    }

    #[derive(Default)]
    struct Inner {
        fsandbox_dictionary: Option<fsandbox::DictionaryRef>,
        lifecycle_control_handle: Option<flifecycle::LifecycleControlHandle>,
        lifecycle_handle: Option<ServerEnd<flifecycle::LifecycleMarker>>,
        watch_for_stop_task: Option<Task<()>>,
        dictionary: Option<Dictionary>,
    }

    impl EscrowOperation {
        /// Creates a new EscrowOperation by taking the process lifecycle handle. Does not actually
        /// perform the escrow operation, see `Self::run`.
        pub fn new() -> Self {
            let lifecycle_handle =
                fruntime::take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0))
                    .expect("No lifecycle channel received, unable to escrow handles");
            let channel: fidl::Channel = lifecycle_handle.into();
            let lifecycle_handle: ServerEnd<flifecycle::LifecycleMarker> = channel.into();
            Self::new_with_lifecycle_handle(lifecycle_handle)
        }

        /// Creates a new EscrowOperation with the provided process lifecycle handle. Does not
        /// actually perform the escrow operation, see `Self::run`.
        pub fn new_with_lifecycle_handle(
            lifecycle_handle: ServerEnd<flifecycle::LifecycleMarker>,
        ) -> Self {
            Self {
                inner: Arc::new(Mutex::new(Inner {
                    lifecycle_handle: Some(lifecycle_handle),
                    ..Default::default()
                })),
            }
        }

        /// Creates a new EscrowOperation with the provided process lifecycle control handle. Does
        /// not actually perform the escrow operation, see `Self::run`.
        pub fn new_with_control_handle(control_handle: flifecycle::LifecycleControlHandle) -> Self {
            Self {
                inner: Arc::new(Mutex::new(Inner {
                    lifecycle_control_handle: Some(control_handle),
                    ..Default::default()
                })),
            }
        }

        /// Adds a dictionary handle to the escrow operation, which will be returned to us when
        /// if/when we are restarted.
        ///
        /// Deprecated, prefer `with_dictionary`.
        pub fn with_fsandbox_dictionary(&self, fsandbox_dictionary: fsandbox::DictionaryRef) {
            self.inner.lock().fsandbox_dictionary = Some(fsandbox_dictionary);
        }

        /// Adds a dictionary handle to the escrow operation, which will be returned to us when
        /// if/when we are restarted.
        pub fn with_dictionary(&self, dictionary: Dictionary) {
            self.inner.lock().dictionary = Some(dictionary);
        }

        /// Starts a new async task that watches the process lifecycle handle for a stop
        /// instruction, and exits this process if/when such a signal is received.
        pub fn watch_for_stop(&self) -> Result<(), Error> {
            let mut inner_guard = self.inner.lock();
            let lifecycle_handle = inner_guard.lifecycle_handle.take().ok_or_else(|| {
                format_err!(
                    "EscrowOperation::wait_for_stop called without setting lifecycle handle"
                )
            })?;
            let (mut stream, control_handle) = lifecycle_handle.into_stream_and_control_handle();
            inner_guard.lifecycle_control_handle = Some(control_handle);
            inner_guard.watch_for_stop_task = Some(Task::spawn(async move {
                let Some(Ok(request)) = stream.next().await else {
                    return;
                };
                match request {
                    flifecycle::LifecycleRequest::Stop { .. } => {
                        // Component manager will never initiate escrow operations. If we're being
                        // asked to stop, then it's not expected of us to escrow our handles. Had
                        // we not asked for our process lifecycle handle then at this point we'd
                        // have our process unceremoniously stopped, so it's fine to do the same
                        // thing manually.
                        //
                        // We use exit code 1 because this function is only used by components that
                        // would not otherwise have a process lifecycle handle were it not a
                        // requirement for escrowing their state, and if they didn't have the
                        // handle they'd be stopped by the ELF runner terminating their process,
                        // which results in a non-zero exit code.
                        process::exit(1);
                    }
                }
            }));
            Ok(())
        }

        /// Runs this escrow operation, sending the outgoing directory and potentially a dictionary
        /// handle to component manager. The component should exit immediately after calling this
        /// function.
        pub fn run(
            &self,
            outgoing_directory: ServerEnd<fio::DirectoryMarker>,
        ) -> Result<(), Error> {
            let mut inner_guard = self.inner.lock();
            let lifecycle_control_handle = match inner_guard.lifecycle_control_handle.take() {
                Some(lifecycle_control_handle) => lifecycle_control_handle,
                None => {
                    let lifecycle_handle = inner_guard
                        .lifecycle_handle
                        .take()
                        .ok_or_else(|| format_err!("EscrowOperation::run called without setting lifecycle handle or control handle"))?;
                    let (_stream, control) = lifecycle_handle.into_stream_and_control_handle();
                    control
                }
            };

            lifecycle_control_handle
                .send_on_escrow(flifecycle::LifecycleOnEscrowRequest {
                    outgoing_dir: Some(outgoing_directory),
                    escrowed_dictionary: inner_guard.fsandbox_dictionary.take(),
                    escrowed_dictionary_handle: inner_guard.dictionary.take().map(|d| d.handle),
                    recoverable_bytes: Some(calculate_recoverable_memory()?),
                    ..Default::default()
                })
                .map_err(|e| format_err!("Failed to escrow handles: {e:?}"))
        }
    }

    /// Returns the number of bytes of memory that we are sure will be reclaimed when this process
    /// exits.
    fn calculate_recoverable_memory() -> Result<u64, Error> {
        // To calculate how much memory will be saved when our component is escrowed, we look
        // at the VMOs that we have handles to. For every process there are some VMOs that are
        // both unable to be paged out, and not shared with any other processes, and will thus
        // be closed when we exit. For example, any scudo and relro VMOs. It's possible that we
        // have handles to other VMOs that are also unpageable and that aren't shared with
        // other components, but it's harder to make generalized assumptions about such VMOs,
        // so we only count VMOs we recognize here to get a confident floor on savings. This
        // calculation also ignores potential savings from kernel page compression.
        let process = fruntime::process_self();
        let vmo_infos = process
            .info_vmos_vec()
            .map_err(|e| format_err!("Failed to inspect own VMOs: {e:?}"))?;
        let vmos_infos_to_include = vmo_infos.into_iter().filter(|vmo_info| {
            vmo_info.name.as_bstr().starts_with(b"scudo")
                || vmo_info.name.as_bstr().starts_with(b"relro")
                || vmo_info.name.as_bstr().starts_with(b"pthread_t")
                || vmo_info.name.as_bstr().starts_with(b"pthread_create")
                || vmo_info.name.as_bstr().starts_with(b"thrd_t:0x")
                || vmo_info.name.as_bstr() == "initial-thread"
        });
        // We collect savings by koid because it's possible that the vmo_infos vector could
        // have duplicates.
        let savings_by_koid = vmos_infos_to_include
            .map(|vmo_info| (vmo_info.koid, vmo_info.committed_private_bytes))
            .collect::<HashSet<_>>();
        let total_recoverable_bytes = savings_by_koid.into_iter().map(|(_, num)| num).sum();
        Ok(total_recoverable_bytes)
    }
}
