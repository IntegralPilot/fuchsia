// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Job;
use crate::config::ElfProgramConfig;
use crate::runtime_dir::RuntimeDirectory;
use async_trait::async_trait;
use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_component_runner::ComponentControllerOnEscrowRequest;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process_lifecycle::{LifecycleEvent, LifecycleProxy};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::future::{BoxFuture, FutureExt};
use futures::stream::BoxStream;
use log::{error, warn};
use moniker::Moniker;
use runner::component::Controllable;
use std::ops::DerefMut;
use std::sync::Arc;
use vfs::ExecutionScope;
use zx::{Process, Task};

/// Immutable information about the component.
///
/// These information is shared with [`crate::ComponentSet`].
pub struct ElfComponentInfo {
    /// Moniker of the ELF component.
    moniker: Moniker,

    /// Instance token of the component.
    component_instance: zx::Event,

    /// Job in which the underlying process that represents the component is
    /// running.
    job: Arc<Job>,

    /// We need to remember if we marked the main process as critical, because if we're asked to
    /// kill a component that has such a marking it'll bring down everything.
    main_process_critical: bool,

    /// URL with which the component was launched.
    component_url: String,

    /// A connection to the component's outgoing directory.
    ///
    /// This option will be present if the ELF runner needs to open capabilities
    /// exposed by the ELF component.
    outgoing_directory_for_memory_attribution: Option<ClientEnd<fio::DirectoryMarker>>,

    /// Configurations from the program block.
    program_config: ElfProgramConfig,
}

impl ElfComponentInfo {
    pub fn get_url(&self) -> &String {
        &self.component_url
    }

    pub fn get_moniker(&self) -> &Moniker {
        &self.moniker
    }

    /// Return a pointer to the Job.
    pub fn copy_job(&self) -> Arc<Job> {
        self.job.clone()
    }

    pub fn get_outgoing_directory(&self) -> Option<&ClientEnd<fio::DirectoryMarker>> {
        self.outgoing_directory_for_memory_attribution.as_ref()
    }

    pub fn get_program_config(&self) -> &ElfProgramConfig {
        &self.program_config
    }

    /// Return a handle to the Job containing the process for this component.
    ///
    /// The rights of the job will be set such that the resulting handle will be appropriate to
    /// use for diagnostics-only purposes. Right now that is ZX_RIGHTS_BASIC (which includes
    /// INSPECT).
    pub fn copy_job_for_diagnostics(&self) -> Result<zx::Job, zx::Status> {
        self.job.top().duplicate_handle(zx::Rights::BASIC)
    }

    pub fn copy_instance_token(&self) -> Result<zx::Event, zx::Status> {
        self.component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS)
    }
}

/// Structure representing a running elf component.
pub struct ElfComponent {
    /// Namespace directory for this component, kept just as a reference to
    /// keep the namespace alive.
    _runtime_dir: RuntimeDirectory,

    /// Immutable information about the component.
    info: Arc<ElfComponentInfo>,

    /// Process made for the program binary defined for this component.
    process: Option<Arc<Process>>,

    /// Client end of the channel given to an ElfComponent which says it
    /// implements the Lifecycle protocol. If the component does not implement
    /// the protocol, this will be None.
    lifecycle_channel: Option<LifecycleProxy>,

    /// Scope that holds tasks to serve this component. For example, stdout and stderr
    /// listeners are Task objects that live for the duration of the component's lifetime.
    ///
    /// Stop will block on these to complete.
    local_scope: ExecutionScope,

    /// A closure to be invoked when the object is dropped.
    on_drop: Mutex<Option<Box<dyn FnOnce(&ElfComponentInfo) + Send + 'static>>>,
}

impl ElfComponent {
    pub fn new(
        _runtime_dir: RuntimeDirectory,
        moniker: Moniker,
        job: Job,
        process: Process,
        lifecycle_channel: Option<LifecycleProxy>,
        main_process_critical: bool,
        local_scope: ExecutionScope,
        component_url: String,
        outgoing_directory: Option<ClientEnd<fio::DirectoryMarker>>,
        program_config: ElfProgramConfig,
        component_instance: zx::Event,
    ) -> Self {
        Self {
            _runtime_dir,
            info: Arc::new(ElfComponentInfo {
                moniker,
                component_instance,
                job: Arc::new(job),
                main_process_critical,
                component_url,
                outgoing_directory_for_memory_attribution: outgoing_directory,
                program_config,
            }),
            process: Some(Arc::new(process)),
            lifecycle_channel,
            local_scope,
            on_drop: Mutex::new(None),
        }
    }

    /// Sets a closure to be invoked when the object is dropped. Can only be done once.
    pub fn set_on_drop(&self, func: impl FnOnce(&ElfComponentInfo) + Send + 'static) {
        let mut on_drop = self.on_drop.lock();
        let previous = std::mem::replace(
            on_drop.deref_mut(),
            Some(Box::new(func) as Box<dyn FnOnce(&ElfComponentInfo) + Send + 'static>),
        );
        assert!(previous.is_none());
    }

    /// Obtain immutable information about the component such as its job and URL.
    pub fn info(&self) -> &Arc<ElfComponentInfo> {
        &self.info
    }

    /// Return a pointer to the Process, returns None if the component has no
    /// Process.
    pub fn copy_process(&self) -> Option<Arc<Process>> {
        self.process.clone()
    }
}

#[async_trait]
impl Controllable for ElfComponent {
    async fn kill(&mut self) {
        if self.info().main_process_critical {
            warn!(
                "killing a component with 'main_process_critical', so this will also kill component_manager and all of its components"
            );
        }
        self.info()
            .job
            .top()
            .kill()
            .unwrap_or_else(|error| error!(error:%; "failed killing job during kill"));
    }

    fn stop<'a>(&mut self) -> BoxFuture<'a, ()> {
        if let Some(lifecycle_chan) = self.lifecycle_channel.take() {
            lifecycle_chan.stop().unwrap_or_else(
                |error| error!(error:%; "failed to stop lifecycle_chan during stop"),
            );

            let job = self.info().job.clone();

            // If the component's main process is critical we must watch for
            // the main process to exit, otherwise we could end up killing that
            // process and therefore killing the root job.
            if self.info().main_process_critical {
                if self.process.is_none() {
                    // This is a bit strange because there's no process, but there is a lifecycle
                    // channel. Since there is no process it seems like killing it can't kill
                    // component manager.
                    warn!(
                        "killing job of component with 'main_process_critical' set because component has lifecycle channel, but no process main process."
                    );
                    self.info().job.top().kill().unwrap_or_else(|error| {
                        error!(error:%; "failed killing job for component with no lifecycle channel")
                    });
                    return async {}.boxed();
                }
                // Try to duplicate the Process handle so we can us it to wait for
                // process termination
                let proc_handle = self.process.take().unwrap();

                async move {
                    fasync::OnSignals::new(
                        &proc_handle.as_handle_ref(),
                        zx::Signals::PROCESS_TERMINATED,
                    )
                    .await
                    .map(|_: fidl::Signals| ()) // Discard.
                    .unwrap_or_else(|e| {
                        error!(
                            "killing component's job after failure waiting on process exit, err: {}",
                            e
                        )
                    });
                    job.top().kill().unwrap_or_else(|error| {
                        error!(error:%; "failed killing job in stop after lifecycle channel closed")
                    });
                }
                .boxed()
            } else {
                async move {
                    lifecycle_chan.on_closed()
                    .await
                    .map(|_: fidl::Signals| ())  // Discard.
                    .unwrap_or_else(|e| {
                        error!(
                        "killing component's job after failure waiting on lifecycle channel, err: {}",
                        e
                        )
                    });
                    job.top().kill().unwrap_or_else(|error| {
                        error!(error:%; "failed killing job in stop after lifecycle channel closed")
                    });
                }
                .boxed()
            }
        } else {
            if self.info().main_process_critical {
                warn!(
                    "killing job of component {} marked with 'main_process_critical' because \
                component does not implement Lifecycle, so this will also kill component_manager \
                and all of its components",
                    self.info().get_url()
                );
            }
            self.info().job.top().kill().unwrap_or_else(|error| {
                error!(error:%; "failed killing job for component with no lifecycle channel")
            });
            async {}.boxed()
        }
    }

    fn teardown<'a>(&mut self) -> BoxFuture<'a, ()> {
        let scope = self.local_scope.clone();
        async move {
            scope.wait().await;
        }
        .boxed()
    }

    fn on_escrow<'a>(&self) -> BoxStream<'a, ComponentControllerOnEscrowRequest> {
        let Some(lifecycle) = &self.lifecycle_channel else {
            return futures::stream::empty().boxed();
        };
        lifecycle
            .take_event_stream()
            .filter_map(|result| async {
                match result {
                    Ok(LifecycleEvent::OnEscrow { payload }) => {
                        Some(ComponentControllerOnEscrowRequest {
                            outgoing_dir: payload.outgoing_dir,
                            escrowed_dictionary: payload.escrowed_dictionary,
                            escrowed_dictionary_handle: payload.escrowed_dictionary_handle,
                            recoverable_bytes: payload.recoverable_bytes,
                            ..Default::default()
                        })
                    }
                    Err(fidl::Error::ClientChannelClosed { .. }) => {
                        // The ELF program is expected to close the server endpoint.
                        None
                    }
                    Err(error) => {
                        warn!(error:%; "error handling lifecycle channel events");
                        None
                    }
                }
            })
            .boxed()
    }
}

impl Drop for ElfComponent {
    fn drop(&mut self) {
        // notify others that this object is being dropped
        if let Some(on_drop) = self.on_drop.lock().take() {
            on_drop(self.info().as_ref());
        }
        // just in case we haven't killed the job already
        self.info()
            .job
            .top()
            .kill()
            .unwrap_or_else(|error| error!(error:%; "failed to kill job in drop"));
    }
}
