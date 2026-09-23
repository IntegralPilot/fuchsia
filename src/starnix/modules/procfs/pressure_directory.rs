// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_starnix_psi::{
    PsiProviderGetMemoryPressureStatsResponse, PsiProviderSynchronousProxy,
    PsiProviderWatchMemoryStallRequest,
};
use fuchsia_async::{OnSignals, WakeupTime};
use fuchsia_component::client::connect_to_protocol_sync;
use futures::{FutureExt, select};
use starnix_core::task::{
    CurrentTask, EventHandler, Kernel, SignalHandler, SignalHandlerInner, WaitCanceler, Waiter,
};
use starnix_core::vfs::buffers::InputBuffer;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectory;
use starnix_core::vfs::pseudo::simple_file::SimpleFileNode;
use starnix_core::vfs::{
    FileObject, FileObjectState, FileOps, FileSystemHandle, FsNodeHandle, FsNodeOps,
    fileops_impl_delegate_read_write_and_seek, fileops_impl_noop_sync,
};
use starnix_logging::{log_error, track_stub};
use starnix_sync::{LockDepMutex, MemoryPressureMonitor, MemoryPressureMonitorClientState};
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{errno, error};

use std::pin::pin;
use std::sync::Arc;

/// Creates the /proc/pressure directory. https://docs.kernel.org/accounting/psi.html
pub fn pressure_directory(kernel: &Kernel, fs: &FileSystemHandle) -> Option<FsNodeHandle> {
    let psi_provider = kernel.expando.get::<PsiProvider>();
    if psi_provider.proxy.is_none() {
        return None;
    };

    let dir = SimpleDirectory::new();
    dir.edit(fs, |dir| {
        dir.entry("memory", MemoryPressureFile::new_node(psi_provider), mode!(IFREG, 0o666));
        dir.entry(
            "cpu",
            StubPressureFile::new_node(StubPressureFileKind::CPU),
            mode!(IFREG, 0o666),
        );
        dir.entry("io", StubPressureFile::new_node(StubPressureFileKind::IO), mode!(IFREG, 0o666));
    });
    // TODO: Validate the mode bits are correct.
    Some(dir.into_node(fs, 0o777))
}

struct PsiProvider {
    proxy: Option<PsiProviderSynchronousProxy>,
}

fn connect_to_psi_provider() -> Option<PsiProviderSynchronousProxy> {
    let proxy = connect_to_protocol_sync::<fidl_fuchsia_starnix_psi::PsiProviderMarker>().ok()?;
    // Our manifest lists PsiProvider as an optional protocol. Let's check if it's
    // connected for real or not.
    let _ = proxy.get_memory_pressure_stats(zx::Instant::INFINITE).ok()?;
    Some(proxy)
}

impl Default for PsiProvider {
    fn default() -> Self {
        Self { proxy: connect_to_psi_provider() }
    }
}

struct MemoryPressureFileSource {
    psi_provider: Arc<PsiProvider>,
    monitor: Arc<LockDepMutex<Option<Arc<PressureMonitorThread>>, MemoryPressureMonitor>>,
}

impl DynamicFileSource for MemoryPressureFileSource {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let PsiProviderGetMemoryPressureStatsResponse { some, full, .. } = self
            .psi_provider
            .proxy
            .as_ref()
            .ok_or_else(|| errno!(EIO))?
            .get_memory_pressure_stats(zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("FIDL error getting memory pressure stats: {e}");
                errno!(EIO)
            })?
            .map_err(|_| errno!(EIO))?;

        let some = some.ok_or_else(|| errno!(EIO))?;
        writeln!(
            sink,
            "some avg10={:.2} avg60={:.2} avg300={:.2} total={}",
            some.avg10.ok_or_else(|| errno!(EIO))?,
            some.avg60.ok_or_else(|| errno!(EIO))?,
            some.avg300.ok_or_else(|| errno!(EIO))?,
            some.total.ok_or_else(|| errno!(EIO))? / 1000
        )?;

        let full = full.ok_or_else(|| errno!(EIO))?;
        writeln!(
            sink,
            "full avg10={:.2} avg60={:.2} avg300={:.2} total={}",
            full.avg10.ok_or_else(|| errno!(EIO))?,
            full.avg60.ok_or_else(|| errno!(EIO))?,
            full.avg300.ok_or_else(|| errno!(EIO))?,
            full.total.ok_or_else(|| errno!(EIO))? / 1000
        )?;

        Ok(())
    }

    /// Pressure notifications are configured by writing to the file.
    fn write(
        &self,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let kernel = current_task.kernel();
        let psi_provider = kernel.expando.get::<PsiProvider>();
        let Some(psi_provider_proxy) = &psi_provider.proxy else {
            return error!(ENOTSUP);
        };

        let data = data.read_all()?;
        let mut param_string = std::str::from_utf8(&data)
            .map_err(|_| errno!(EINVAL))?
            .trim_matches('\n')
            .split_whitespace();

        let mode = param_string.next().ok_or_else(|| errno!(EINVAL))?;
        let threshold_ms = param_string
            .next()
            .and_then(|t| t.parse::<i64>().ok())
            .ok_or_else(|| errno!(EINVAL))?;
        let threshold = zx::MonotonicDuration::from_micros(threshold_ms);
        let window_ms = param_string
            .next()
            .and_then(|w| w.trim_matches(char::from(0)).parse::<i64>().ok())
            .ok_or_else(|| errno!(EINVAL))?;
        let window = zx::MonotonicDuration::from_micros(window_ms);

        let kind = match mode {
            "some" => zx::sys::ZX_SYSTEM_MEMORY_STALL_SOME,
            "full" => zx::sys::ZX_SYSTEM_MEMORY_STALL_FULL,
            _ => {
                return error!(EINVAL);
            }
        };

        let mut monitor = self.monitor.lock();
        if monitor.is_some() {
            return error!(EBUSY);
        }

        let zircon_event = match psi_provider_proxy.watch_memory_stall(
            &PsiProviderWatchMemoryStallRequest {
                kind: Some(kind),
                threshold: Some(threshold.into_nanos()),
                window: Some(window.into_nanos()),
                ..Default::default()
            },
            zx::MonotonicInstant::INFINITE,
        ) {
            Ok(Ok(response)) => response.event.unwrap(),
            e => {
                log_error!("error watching memory stall: {:?}", e);
                return error!(EINVAL);
            }
        };

        *monitor = Some(PressureMonitorThread::new(&current_task.kernel(), zircon_event, window));

        Ok(data.len())
    }
}

struct MemoryPressureFile {
    source: DynamicFile<MemoryPressureFileSource>,
    monitor: Arc<LockDepMutex<Option<Arc<PressureMonitorThread>>, MemoryPressureMonitor>>,
}

impl MemoryPressureFile {
    pub fn new_node(psi_provider: Arc<PsiProvider>) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| {
            let monitor = Arc::new(LockDepMutex::new(None));
            Ok(Self {
                source: DynamicFile::new(MemoryPressureFileSource {
                    psi_provider: psi_provider.clone(),
                    monitor: monitor.clone(),
                }),
                monitor,
            })
        })
    }
}

impl FileOps for MemoryPressureFile {
    fileops_impl_delegate_read_write_and_seek!(self, self.source);
    fileops_impl_noop_sync!();

    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        if let Some(monitor) = self.monitor.lock().take() {
            monitor.stop();
        }
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        _events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let monitor = self.monitor.lock();
        let Some(ref monitor) = *monitor else {
            return None;
        };

        // Create a new event and start waiting for it.
        let event = zx::Event::create();
        let signal_handler = SignalHandler {
            inner: SignalHandlerInner::ZxHandle(get_events_from_signals),
            event_handler: handler,
            err_code: None,
        };
        let canceler = waiter
            .wake_on_zircon_signals(&event, zx::Signals::EVENT_SIGNALED, signal_handler)
            .unwrap();
        let result = WaitCanceler::new_port(canceler);

        // Update the notification state.
        monitor.client_state.lock().mutate_in_place(|old_value| match old_value {
            // If a signal has already been latched, deliver it immediately and start a new cycle.
            PressureMonitorClientState::PressureLatched => {
                event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();
                PressureMonitorClientState::Idle
            }
            // Otherwise, ask to be notified in the future.
            // If a client was already registered, we steal its spot. (This should not really happen
            // given our assumption that clients are single threaded, see comment in query_events).
            PressureMonitorClientState::Idle
            | PressureMonitorClientState::WaitingForPressure { .. } => {
                PressureMonitorClientState::WaitingForPressure { target_event: event }
            }
        });

        Some(result)
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let mut events = FdEvents::POLLIN | FdEvents::POLLOUT;
        let monitor = self.monitor.lock();
        if let Some(ref monitor) = *monitor {
            // Note: the following logic assumes that userspace will never poll from more than one
            // thread at the same time. We'll need to revisit it if it stops being the case.
            match &*monitor.client_state.lock() {
                // The previous cycle has ended and userspace has not yet started a new wait.
                // Therefore, from the userspace point of view, we're still in the previous cycle.
                // Let's act accordingly and report POLLPRI.
                PressureMonitorClientState::Idle | PressureMonitorClientState::PressureLatched => {
                    events |= FdEvents::POLLPRI;
                }
                // Userspace has started a new wait and no event has happened yet.
                PressureMonitorClientState::WaitingForPressure { target_event: _ } => {}
            }
        }
        Ok(events)
    }
}

/// In order to match the expected Linux PSI interface, the Zircon stall signal needs to be latched
/// and rate-limited. We do so in a dedicated "monitor" thread that performs this arbitration.
/// This struct contains data that is shared by the monitor thread and its clients.
struct PressureMonitorThread {
    /// The state of the notification state machine.
    client_state: LockDepMutex<PressureMonitorClientState, MemoryPressureMonitorClientState>,

    /// Signaled when the PSI file descriptor is closed to ask the kthread to quit.
    should_stop: zx::Event,
}

/// This enum contains the state of the current notification cycle.
///
/// Cycles correspond to the delivery of a single pressure notification event to userspace.
///
/// When a cycle starts, the pressure event has not happened yet and no client has registered. Then,
/// depending on what happens first (the pressure event or the client registration), the cycle will
/// enter the corresponding intermediate state. Finally, as soon as both conditions have happened,
/// the event is delivered to the client and a new cycle immediately starts.
///
/// The rate-limiting performed by the monitor thread ensures that each cycle lasts at least as
/// long as the configured window.
enum PressureMonitorClientState {
    /// Neither the client has registered nor the event has happened yet.
    Idle,

    /// No client has registered yet but this cycle's pressure event has already happened.
    ///
    /// In this state, we remember the event so it can be notified immediately as soon as the client
    /// registers.
    PressureLatched,

    /// A client has registered, but this cycle's pressure event has not happened yet.
    ///
    /// In this state, we remember the handle to notify the client, so we can signal it as soon the
    /// pressure event happens.
    WaitingForPressure { target_event: zx::Event },
}

impl PressureMonitorThread {
    fn new(
        kernel: &Kernel,
        zircon_stall_event: zx::Event,
        rate_limiting_window: zx::MonotonicDuration,
    ) -> Arc<PressureMonitorThread> {
        let result = Arc::new(PressureMonitorThread {
            client_state: LockDepMutex::new(PressureMonitorClientState::Idle),
            should_stop: zx::Event::create(),
        });

        // Start the monitor kthread.
        kernel.kthreads.spawn_future(
            {
                let result = Arc::clone(&result);
                move || async move { result.worker(zircon_stall_event, rate_limiting_window).await }
            },
            "pressure_monitor",
        );

        result
    }

    fn stop(&self) {
        self.should_stop.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();
    }

    async fn worker(
        self: Arc<Self>,
        zircon_stall_event: zx::Event,
        rate_limiting_window: zx::MonotonicDuration,
    ) {
        let mut should_stop = pin!(
            OnSignals::new(
                self.should_stop.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                zx::Signals::EVENT_SIGNALED,
            )
            .fuse()
        );

        loop {
            // Wait for the Zircon event to be signaled.
            let mut zircon_stall_event = pin!(
                OnSignals::new(
                    zircon_stall_event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    zx::Signals::EVENT_SIGNALED,
                )
                .fuse()
            );
            select! {
                _ = zircon_stall_event => (),
                _ = should_stop => return,
            }

            // Start counting until the end of the rate-limiting window.
            let mut rate_limiting_timer = pin!(rate_limiting_window.into_timer());

            // Notify.
            self.client_state.lock().mutate_in_place(|old_value| match old_value {
                // If a client is currently waiting, notify it and start a new cycle.
                PressureMonitorClientState::WaitingForPressure { target_event } => {
                    target_event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();
                    PressureMonitorClientState::Idle
                }
                // Otherwise, just remember that the pressure event has already happened.
                // If a pressure event was already latched, it will be folded into the new one
                // and a single notification will be delivered to userspace.
                PressureMonitorClientState::Idle | PressureMonitorClientState::PressureLatched => {
                    PressureMonitorClientState::PressureLatched
                }
            });

            // Wait for the next rate-limiting window.
            select! {
                _ = rate_limiting_timer => (),
                _ = should_stop => return,
            }
        }
    }
}

impl PressureMonitorClientState {
    fn mutate_in_place(
        &mut self,
        mutator: impl FnOnce(PressureMonitorClientState) -> PressureMonitorClientState,
    ) {
        // Take the old value and temporarily swap it with a placeholder value.
        const PLACEHOLDER: PressureMonitorClientState = PressureMonitorClientState::Idle;
        let old_value = std::mem::replace(self, PLACEHOLDER);

        // Store the real new value.
        let new_value = mutator(old_value);
        *self = new_value;
    }
}

fn get_events_from_signals(signals: zx::Signals) -> FdEvents {
    if signals.contains(zx::Signals::EVENT_SIGNALED) {
        FdEvents::POLLPRI
    } else {
        FdEvents::empty()
    }
}

struct StubPressureFileSource {
    kind: StubPressureFileKind,
}

impl DynamicFileSource for StubPressureFileSource {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        writeln!(sink, "some avg10=0.00 avg60=0.00 avg300=0.00 total=0")?;
        writeln!(sink, "full avg10=0.00 avg60=0.00 avg300=0.00 total=0")?;
        Ok(())
    }

    /// Pressure notifications are configured by writing to the file.
    fn write(
        &self,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        // Ignore the request for now.

        track_stub!(
            TODO("https://fxbug.dev/322873423"),
            match self.kind {
                StubPressureFileKind::CPU => "cpu pressure notification setup",
                StubPressureFileKind::IO => "io pressure notification setup",
            }
        );
        Ok(data.drain())
    }
}

#[derive(Clone, Copy)]
enum StubPressureFileKind {
    CPU,
    IO,
}

struct StubPressureFile(DynamicFile<StubPressureFileSource>);

impl StubPressureFile {
    pub fn new_node(kind: StubPressureFileKind) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| Ok(Self(DynamicFile::new(StubPressureFileSource { kind }))))
    }
}

impl FileOps for StubPressureFile {
    fileops_impl_delegate_read_write_and_seek!(self, self.0);
    fileops_impl_noop_sync!();

    fn wait_async(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        if let Ok(current_events) = self.query_events(file, current_task) {
            let events = events.intersection(current_events);
            if !events.is_empty() {
                handler.handle(events);
            }
        }
        Some(waiter.fake_wait())
    }
}
