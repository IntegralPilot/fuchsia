// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod element;
mod suspension_block;
mod transmit_suspension_handler;

use std::collections::HashMap;
use std::pin::pin;

use fidl::endpoints::{ControlHandle as _, Responder as _};
use fidl_fuchsia_power_system as fpower_system;
use futures::channel::mpsc;
use futures::future::{Future, OptionFuture};
use futures::{FutureExt as _, Stream, StreamExt as _};
use log::{error, warn};
use thiserror::Error;

use element::InternalLessor;
use suspension_block::{SuspensionBlock, SuspensionBlockControl};
pub(crate) use transmit_suspension_handler::TransmitSuspensionHandler;

use crate::bindings::power::element::InternalLease;
use crate::bindings::util::ResultExt;

pub(crate) struct PowerWorker {
    work_stream: mpsc::UnboundedReceiver<SinkWork>,
    suspension_control: SuspensionBlockControl,
}

impl PowerWorker {
    pub(crate) fn new() -> (Self, PowerWorkerSink) {
        let (work_sink, work_stream) = mpsc::unbounded();
        let suspension_control = SuspensionBlockControl::new();
        let suspension_block = suspension_control.issuer();
        let this = Self { work_stream, suspension_control };
        (this, PowerWorkerSink { work_sink, suspension_block })
    }

    pub(crate) async fn run(self, suspend_enabled: bool) {
        let stream = if suspend_enabled {
            futures::stream::unfold((), async |()| match open_suspension_blocker().await {
                Ok(s) => Some((s, ())),
                Err(e) => {
                    error!(
                        "failed to register suspend blocker, \
                        proceeding with suspension disabled: {e}"
                    );
                    None
                }
            })
            .left_stream()
        } else {
            futures::stream::empty().right_stream()
        };
        self.run_inner(stream).await
    }

    async fn run_inner(
        self,
        suspend_blocker_stream: impl Stream<Item = fpower_system::SuspendBlockerRequestStream>,
    ) {
        let Self { work_stream, suspension_control } = self;

        enum Work {
            Sink(SinkWork),
            SuspendBlockerFinished,
            SuspendBlockerRequest(fpower_system::SuspendBlockerRequest),
            SuspensionComplete,
        }

        // We receive a stream of request streams from the caller. Whenever a
        // stream finishes, we notify the loop so we go back to the expected
        // resumed state. This allows us to drop connections with SAG if we find
        // ourselves in a bad state as well as offering an always-pending stream
        // when suspend is disabled.
        let mut suspend_blocker_stream = pin!(
            suspend_blocker_stream
                .map(|rs| {
                    rs.filter_map(|r| {
                        futures::future::ready(match r {
                            Ok(r) => Some(Work::SuspendBlockerRequest(r)),
                            Err(e) => {
                                // Filter out these errors, assuming the FIDL channel will
                                // close and we'll observe the finished state.
                                error!("error from suspend blocker stream: {e}");
                                None
                            }
                        })
                    })
                    .chain(futures::stream::once(async { Work::SuspendBlockerFinished }))
                })
                .flatten()
                .fuse()
        );

        let mut work_stream = work_stream.map(Work::Sink);

        let mut worker_state = PowerWorkerInner {
            suspension_control,
            state: SuspensionState::Active,
            lessors: Default::default(),
        };
        let mut suspension_fut = pin!(OptionFuture::default());
        loop {
            let work = futures::select! {
                w = work_stream.next() => w.expect("power worker sink closed unexpectedly"),
                // Our stream of streams can be closed if we fail to connect to
                // the service. We don't need to act on this here as the
                // SuspendBlockerFinished variant performs the work of putting
                // us back in the desired state.
                w = suspend_blocker_stream.select_next_some() => w,
                o = suspension_fut => {
                    o.expect("option future doesn't yield in select");
                    Work::SuspensionComplete
                }
            };
            match work {
                Work::Sink(SinkWork::RegisterInternalLessor(lessor)) => {
                    worker_state.register_lessor(lessor);
                }
                Work::Sink(SinkWork::UnregisterInternalLessor(lessor)) => {
                    worker_state.unregister_lessor(lessor);
                }
                Work::SuspensionComplete => {
                    worker_state.suspension_complete();
                }
                Work::SuspendBlockerRequest(r) => match r {
                    fpower_system::SuspendBlockerRequest::BeforeSuspend { responder } => {
                        match worker_state.suspend_requested(responder) {
                            Ok(fut) => {
                                suspension_fut.set(Some(fut.fuse()).into());
                            }
                            Err((e, responder)) => {
                                error!("bad BeforeSuspend request: {e:?}");
                                // Shutdown this channel after a bad request.
                                responder.control_handle().shutdown();
                            }
                        }
                    }
                    fpower_system::SuspendBlockerRequest::AfterResume { responder } => {
                        match worker_state.resume_requested() {
                            Ok(()) => {
                                suspension_fut.set(None.into());
                                responder.send().unwrap_or_log("responding to AfterResume");
                            }
                            Err(e) => {
                                error!("bad AfterResume request: {e:?}");
                                // Shutdown this channel after a bad request.
                                responder.control_handle().shutdown();
                            }
                        }
                    }
                    fpower_system::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                        warn!("received unknown ordinal {ordinal} on SuspendBlocker");
                    }
                },
                Work::SuspendBlockerFinished => {
                    // We should not be observing the suspend blocker ever
                    // finishing. If we do, something went wrong. To ensure logs
                    // can be extracted from the system in that scenario, make
                    // sure we come back as if a resume was requested.
                    error!("suspend blocker stream terminated, returning to resumed state");
                    // Resume unconditionally without checking valid state.
                    worker_state.resume();
                    suspension_fut.set(None.into());
                }
            }
        }
    }
}

async fn open_suspension_blocker()
-> Result<fpower_system::SuspendBlockerRequestStream, PowerFrameworkError> {
    let sag =
        fuchsia_component::client::connect_to_protocol::<fpower_system::ActivityGovernorMarker>()
            .map_err(PowerFrameworkError::ConnectToProtocol)?;
    let (client_end, server_end) = fidl::endpoints::create_endpoints();
    let lease_token = sag
        .register_suspend_blocker(fpower_system::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(client_end),
            name: Some("netstack".to_string()),
            ..Default::default()
        })
        .await??;

    // We don't need to keep the system awake after registering.
    drop(lease_token);
    Ok(server_end.into_stream())
}

/// Inner state kept by [`PowerWorker::run`].
///
/// Extracted into a structure so we can pull the interesting methods out of
/// deep indentation.
struct PowerWorkerInner {
    suspension_control: SuspensionBlockControl,
    state: SuspensionState,
    lessors: HashMap<InternalLessor, Option<InternalLease>>,
}

impl PowerWorkerInner {
    fn register_lessor(&mut self, lessor: InternalLessor) {
        let lease = match &self.state {
            SuspensionState::Suspended | SuspensionState::PendingSuspension(_) => None,
            SuspensionState::Active => Some(lessor.lease()),
        };
        // We can't register the same lessor twice.
        assert!(self.lessors.insert(lessor, lease).is_none());
    }

    fn unregister_lessor(&mut self, lessor: InternalLessor) {
        // May not unregister a lessor that was not previously registered.
        assert!(self.lessors.remove(&lessor).is_some());
    }

    fn suspend_requested(
        &mut self,
        responder: fpower_system::SuspendBlockerBeforeSuspendResponder,
    ) -> Result<
        impl Future<Output = ()> + use<>,
        (BadSuspensionStateError, fpower_system::SuspendBlockerBeforeSuspendResponder),
    > {
        let Self { suspension_control, state, lessors } = self;
        match state {
            s @ SuspensionState::Suspended | s @ SuspensionState::PendingSuspension(_) => {
                return Err((s.to_err(), responder));
            }
            SuspensionState::Active => {}
        }

        *state = SuspensionState::PendingSuspension(responder);
        // Drop all the leases.
        for lease in lessors.values_mut() {
            *lease = None;
        }
        Ok(suspension_control.suspend())
    }

    fn resume_requested(&mut self) -> Result<(), BadSuspensionStateError> {
        let Self { suspension_control: _, state, lessors: _ } = self;
        match state {
            s @ SuspensionState::PendingSuspension(_) | s @ SuspensionState::Active => {
                return Err(s.to_err());
            }
            SuspensionState::Suspended => {}
        }
        self.resume();
        Ok(())
    }

    fn resume(&mut self) {
        let Self { suspension_control, state, lessors } = self;
        *state = SuspensionState::Active;
        // Allow suspension blocks to be acquired again.
        suspension_control.resume();
        // Reacquire all the leases.
        for (lessor, lease) in lessors.iter_mut() {
            *lease = Some(lessor.lease());
        }
    }

    fn suspension_complete(&mut self) {
        let Self { suspension_control: _, state, lessors: _ } = self;
        let prev = std::mem::replace(state, SuspensionState::Suspended);
        match prev {
            SuspensionState::PendingSuspension(responder) => {
                responder.send().unwrap_or_log("responding to BeforeSuspend");
            }
            s @ SuspensionState::Suspended | s @ SuspensionState::Active => {
                panic!("suspension completed in bad state {s:?}")
            }
        }
    }
}

#[derive(Debug)]
enum BadSuspensionStateError {
    Suspended,
    PendingSuspension,
    Active,
}

#[derive(Debug)]
enum SuspensionState {
    Suspended,
    PendingSuspension(fpower_system::SuspendBlockerBeforeSuspendResponder),
    Active,
}

impl SuspensionState {
    fn to_err(&self) -> BadSuspensionStateError {
        match self {
            Self::Suspended => BadSuspensionStateError::Suspended,
            Self::PendingSuspension(_) => BadSuspensionStateError::PendingSuspension,
            Self::Active => BadSuspensionStateError::Active,
        }
    }
}

/// Provides communication from other modules to the [`PowerWorker`].
pub(crate) struct PowerWorkerSink {
    work_sink: mpsc::UnboundedSender<SinkWork>,
    suspension_block: SuspensionBlock,
}

impl PowerWorkerSink {
    /// Register an [`InternalLessor`] that must be leased whenever
    /// [`PowerWorker`] sees that the system is awake from suspension.
    ///
    /// The returned [`LessorRegistration`] unregisters the `lessor` on drop.
    ///
    /// Registering the same lessor (for the same power element) is invalid and
    /// causes a panic in [`PowerWorker`].
    pub(crate) fn register_internal_lessor(&self, lessor: InternalLessor) -> LessorRegistration {
        let registration =
            LessorRegistration { lessor: lessor.clone(), sink: self.work_sink.clone() };
        self.work_sink
            .unbounded_send(SinkWork::RegisterInternalLessor(lessor))
            .expect("power worker not running");
        registration
    }
}

/// A lessor registration within the [`PowerWorker`].
pub(crate) struct LessorRegistration {
    lessor: InternalLessor,
    sink: mpsc::UnboundedSender<SinkWork>,
}

impl Drop for LessorRegistration {
    fn drop(&mut self) {
        let Self { lessor, sink } = self;
        sink.unbounded_send(SinkWork::UnregisterInternalLessor(lessor.clone()))
            .expect("power worker not running");
    }
}

enum SinkWork {
    RegisterInternalLessor(InternalLessor),
    UnregisterInternalLessor(InternalLessor),
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub(crate) enum BinaryPowerLevel {
    Off,
    On,
}

impl BinaryPowerLevel {
    pub(crate) fn flip(&self) -> Self {
        match self {
            BinaryPowerLevel::On => BinaryPowerLevel::Off,
            BinaryPowerLevel::Off => BinaryPowerLevel::On,
        }
    }
}

#[derive(Error, Debug)]
enum PowerFrameworkError {
    #[error("failed to connect to protocol: {0}")]
    ConnectToProtocol(#[source] anyhow::Error),
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    #[error("failed to register suspend blocker: {0:?}")]
    RegisterSuspendBlocker(fpower_system::RegisterSuspendBlockerError),
}

impl From<fpower_system::RegisterSuspendBlockerError> for PowerFrameworkError {
    fn from(e: fpower_system::RegisterSuspendBlockerError) -> Self {
        Self::RegisterSuspendBlocker(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::task::Poll;

    use assert_matches::assert_matches;
    use fixture::fixture;
    use fuchsia_async as fasync;

    use crate::bindings::power::element::InternalPowerElement;
    use crate::bindings::power::suspension_block::SuspensionGuard;

    struct FakeDeps {
        suspend_blocker_sink: mpsc::UnboundedSender<fpower_system::SuspendBlockerRequestStream>,
        suspend_blocker: fpower_system::SuspendBlockerProxy,
    }

    impl FakeDeps {
        fn new() -> (Self, PowerWorkerSink, fasync::Task<()>) {
            let (worker, sink) = PowerWorker::new();
            let (proxy, stream) =
                fidl::endpoints::create_proxy_and_stream::<fpower_system::SuspendBlockerMarker>();
            let (suspend_blocker_sink, suspend_blocker_stream) = mpsc::unbounded();
            suspend_blocker_sink.unbounded_send(stream).expect("stream is open");
            let fake_deps = FakeDeps { suspend_blocker_sink, suspend_blocker: proxy };
            let task = fasync::Task::spawn(
                worker
                    .run_inner(suspend_blocker_stream)
                    .map(|()| panic!("worker finished unexpectedly")),
            );
            (fake_deps, sink, task)
        }

        fn new_suspend_blocker(&mut self) {
            let (proxy, stream) =
                fidl::endpoints::create_proxy_and_stream::<fpower_system::SuspendBlockerMarker>();
            self.suspend_blocker = proxy;
            self.suspend_blocker_sink.unbounded_send(stream).expect("stream is open");
        }
    }

    async fn with_worker<Fut: Future<Output = ()>, F: FnOnce(FakeDeps, PowerWorkerSink) -> Fut>(
        _name: &str,
        f: F,
    ) {
        let (fake_deps, sink, _task) = FakeDeps::new();
        f(fake_deps, sink).await;
    }

    fn with_worker_and_executor<F: FnOnce(FakeDeps, PowerWorkerSink, &mut fasync::TestExecutor)>(
        _name: &str,
        f: F,
    ) {
        let mut executor = fasync::TestExecutor::new();
        let (fake_deps, sink, _task) = FakeDeps::new();
        f(fake_deps, sink, &mut executor);
    }

    #[fixture(with_worker_and_executor)]
    #[test]
    fn blocking_suspension(deps: FakeDeps, sink: PowerWorkerSink, exec: &mut fasync::TestExecutor) {
        let guard = sink.suspension_block.try_guard().expect("acquire guard");
        let mut fut = pin!(deps.suspend_blocker.before_suspend());
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        drop(guard);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn suspend_resume(deps: FakeDeps, sink: PowerWorkerSink) {
        let mut pe = InternalPowerElement::new();
        let _reg = sink.register_internal_lessor(pe.lessor());
        let guard = sink.suspension_block.try_guard().expect("acquire guard");
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
        futures::future::join(
            deps.suspend_blocker.before_suspend().map(|r| r.expect("calling before suspend")),
            async {
                assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::Off);
                drop(guard);
            },
        )
        .await;

        let (r, level, guard) = futures::future::join3(
            deps.suspend_blocker.after_resume(),
            pe.wait_level_change(),
            sink.suspension_block.guard(),
        )
        .await;
        r.expect("calling after resume");
        assert_eq!(level, BinaryPowerLevel::On);
        let _: SuspensionGuard = guard;
    }

    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn drop_registration(_deps: FakeDeps, sink: PowerWorkerSink) {
        let mut pe = InternalPowerElement::new();
        let reg = sink.register_internal_lessor(pe.lessor());
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
        drop(reg);
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::Off);
        // Can register the same one again.
        let _reg = sink.register_internal_lessor(pe.lessor());
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
    }

    #[fixture(with_worker_and_executor)]
    #[test]
    fn register_pe_when_suspended(
        deps: FakeDeps,
        sink: PowerWorkerSink,
        exec: &mut fasync::TestExecutor,
    ) {
        let mut pe = InternalPowerElement::new();
        exec.run_singlethreaded(deps.suspend_blocker.before_suspend()).expect("before suspend");
        let _reg = sink.register_internal_lessor(pe.lessor());
        let mut fut = pin!(pe.wait_level_change());
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);
        exec.run_singlethreaded(deps.suspend_blocker.after_resume()).expect("after resume");
        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Ready(BinaryPowerLevel::On));
    }

    // Tests that receiving unexpected signals from PF still allows us to march
    // forward.
    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn unexpected_client_calls(mut deps: FakeDeps, sink: PowerWorkerSink) {
        let guard = sink.suspension_block.try_guard().expect("acquire guard");
        let mut pe = InternalPowerElement::new();
        let _reg = sink.register_internal_lessor(pe.lessor());
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);

        // The system should be in resumed state, so if we send after resume
        // again we should see the channel close.
        assert_matches!(deps.suspend_blocker.after_resume().await, Err(e) if e.is_closed());
        deps.new_suspend_blocker();

        // Resume while pending suspension.
        let mut request_suspension = deps.suspend_blocker.before_suspend();
        futures::select! {
            l = pe.wait_level_change().fuse() => assert_eq!(l, BinaryPowerLevel::Off),
            r = request_suspension => panic!("unexpected response: {r:?}"),
        };
        assert_matches!(deps.suspend_blocker.after_resume().await, Err(e) if e.is_closed());
        assert_matches!(request_suspension.await, Err(e) if e.is_closed());
        // System resumes while waiting for the next connection.
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
        deps.new_suspend_blocker();

        // New suspend while pending suspension.
        let mut request_suspension = deps.suspend_blocker.before_suspend();
        futures::select! {
            l = pe.wait_level_change().fuse() => assert_eq!(l, BinaryPowerLevel::Off),
            r = request_suspension => panic!("unexpected response: {r:?}"),
        };
        assert_matches!(deps.suspend_blocker.before_suspend().await, Err(e) if e.is_closed());
        assert_matches!(request_suspension.await, Err(e) if e.is_closed());
        // System resumes while waiting for the next connection.
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
        deps.new_suspend_blocker();

        // Can't call suspend when already suspended.
        drop(guard);
        deps.suspend_blocker.before_suspend().await.expect("before suspend");
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::Off);
        assert_matches!(deps.suspend_blocker.before_suspend().await, Err(e) if e.is_closed());
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
        deps.new_suspend_blocker();

        // Dropping our side of the channel causes resume, including closing the
        // connector stream.
        deps.suspend_blocker.before_suspend().await.expect("before suspend");
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::Off);
        let FakeDeps { suspend_blocker_sink, suspend_blocker } = deps;
        drop((suspend_blocker_sink, suspend_blocker));
        assert_eq!(pe.wait_level_change().await, BinaryPowerLevel::On);
        let _guard = sink.suspension_block.try_guard().expect("acquire guard");
    }
}
