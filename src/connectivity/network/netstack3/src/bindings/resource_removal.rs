// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The worker that supports deferred resource removal in bindings.

use std::time::Duration;

use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::stream::SelectAll;
use futures::{Future, FutureExt as _, Stream, StreamExt};
use log::{debug, error, info, warn};
use netstack3_core::sync::DynDebugReferences;

use crate::bindings::reference_notifier::ReferenceReceiver;

/// The interval at which [`ResourceRemovalWorker`] generates reports for each
/// pending resource.
///
/// This is a large enough number to avoid too much log spam and false positives
/// in CQ where timing is not always reliable.
const ACTION_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_minutes(1);
/// The interval count threshold after which we consider the resource stuck.
///
/// This is a large enough number to avoid generating false positive error log
/// lines in CQ where timing is not always reliable.
const STUCK_THRESHOLD: usize = 10;

pub(crate) struct ResourceRemovalWorker {
    receiver: mpsc::UnboundedReceiver<ResourceItem>,
}

impl ResourceRemovalWorker {
    pub(crate) fn new() -> (Self, ResourceRemovalSink) {
        let (sender, receiver) = mpsc::unbounded();
        (Self { receiver }, ResourceRemovalSink { sender })
    }

    pub(crate) async fn run(self) {
        self.run_with_handler(&LoggingHandler {}).await
    }

    async fn run_with_handler<H: ActionHandler>(self, handler: &H) {
        let Self { mut receiver } = self;
        let mut pending = SelectAll::new();

        enum Action {
            Item(ResourceItem),
            StreamResult(StreamResult),
        }

        let report_stream_result = |result| match result {
            StreamResult::Done { created, item } => handler
                .report_item(State::Finished(fasync::MonotonicInstant::now() - created), &item),
            StreamResult::Pending { count, created, item } => {
                let duration = fasync::MonotonicInstant::now() - created;
                let state = if count >= STUCK_THRESHOLD {
                    State::Stuck(duration)
                } else {
                    State::Pending(duration)
                };
                handler.report_item(state, &item);
            }
        };

        loop {
            let action = futures::select! {
                r = receiver.next() => match r {
                    Some(i) => Action::Item(i),
                    // Sink is closed, shutdown the worker.
                    None => break,
                },
                i = pending.next() => {
                    match i {
                        Some(i) => Action::StreamResult(i),
                        // SelectAll implements FusedStream, the select macro
                        // will take care of not polling it when empty.
                        None => continue,
                    }
                }
            };
            match action {
                Action::Item(item) => {
                    handler.report_item(State::Started, &item.info);
                    pending.push(item_stream(item))
                }
                Action::StreamResult(r) => {
                    report_stream_result(r);
                }
            }
        }

        // Give all pending streams a last shot at resolving before panicking.
        // Given reference notifiers will panic if the receivers are closed, if
        // we have unresolved receivers it means a panic could happen elsewhere
        // and it's better to have them here where we have more information.
        //
        // Note that this will stall anything waiting for this task to finish
        // until we hit the interval and stuck threshold, which is a feature to
        // avoid this panic in CQ.
        pending
            .for_each(|r| {
                match &r {
                    StreamResult::Pending { count, created: _, item } => {
                        if *count >= STUCK_THRESHOLD {
                            panic!(
                                "resource removal worker closed with \
                                stuck pending resource: {item:?}"
                            );
                        }
                    }
                    StreamResult::Done { .. } => (),
                }
                report_stream_result(r);
                futures::future::ready(())
            })
            .await;
    }
}

enum StreamResult {
    Done { created: fasync::MonotonicInstant, item: ResourceItemInfo },
    Pending { count: usize, created: fasync::MonotonicInstant, item: ResourceItemInfo },
}

fn item_stream(item: ResourceItem) -> impl Stream<Item = StreamResult> {
    let ResourceItem { fut, info } = item;
    let created = fasync::MonotonicInstant::now();
    let interval = fasync::Interval::new(ACTION_INTERVAL);
    let mut count = 0;
    let last = StreamResult::Done { created, item: info.clone() };
    interval
        .map(move |()| {
            count += 1;
            StreamResult::Pending { count, created, item: info.clone() }
        })
        .take_until(fut)
        .chain(futures::stream::once(futures::future::ready(last)))
}

#[derive(Debug, Clone)]
// Expect unused because this structure is used as a container for printing
// debug information.
#[expect(unused)]
struct ResourceItemInfo {
    typename: &'static str,
    debug_refs: DynDebugReferences,
    #[cfg(feature = "instrumented")]
    location: std::panic::Location<'static>,
}

struct ResourceItem {
    fut: BoxFuture<'static, ()>,
    info: ResourceItemInfo,
}

pub(crate) struct ResourceRemovalSink {
    sender: mpsc::UnboundedSender<ResourceItem>,
}

impl ResourceRemovalSink {
    #[cfg_attr(feature = "instrumented", track_caller)]
    pub(crate) fn defer_removal<
        T: Send + 'static,
        F: Future<Output = T> + Send + Sync + 'static,
        CB: FnOnce(T) + Send + 'static,
    >(
        &self,
        debug_refs: DynDebugReferences,
        fut: F,
        callback: CB,
    ) {
        // Map the future with the callback and box it so we can type-erase it.
        // Deferred resource removal doesn't happen in the fast path and we can
        // deal with the extra allocation here in exchange for lower code
        // complexity.
        let fut = fut.map(callback).boxed();
        // We do use the T parameter to extract some debug information.
        let typename = std::any::type_name::<T>();

        // Send the item to the worker and panic if we can't. The worker should
        // be guaranteed to be alive for as long as we can have deferred
        // resource removal. We must not drop receivers on the floor.
        self.sender
            .unbounded_send(ResourceItem {
                fut,
                info: ResourceItemInfo {
                    typename,
                    debug_refs,
                    #[cfg(feature = "instrumented")]
                    location: *std::panic::Location::caller(),
                },
            })
            .expect("worker not running");
    }

    #[cfg_attr(feature = "instrumented", track_caller)]
    pub(crate) fn defer_removal_with_receiver<T: Send + 'static>(
        &self,
        receiver: ReferenceReceiver<T>,
    ) {
        let ReferenceReceiver { receiver, debug_references } = receiver;
        self.defer_removal(
            debug_references,
            receiver.map(|r| r.expect("sender dropped without notifying receiver")),
            |_: T| {},
        );
    }

    pub(crate) fn close(&self) {
        self.sender.close_channel();
    }
}

#[cfg_attr(test, derive(Debug, Eq, PartialEq, Copy, Clone))]
enum State {
    Started,
    Pending(zx::MonotonicDuration),
    Stuck(zx::MonotonicDuration),
    Finished(zx::MonotonicDuration),
}

/// A trait abstracting the actions taken by [`ResourceRemovalWorker`] to
/// support testing.
trait ActionHandler {
    fn report_item(&self, state: State, info: &ResourceItemInfo);
}

/// A type that implements [`ActionHandler`].
///
/// It provides the default production implementation of
/// [`ResourceRemovalWorker`]'s behavior.
struct LoggingHandler {}

impl ActionHandler for LoggingHandler {
    fn report_item(&self, state: State, info: &ResourceItemInfo) {
        // Standard library duration prints prettier than zx. Unwrap
        // shouldn't be hit because all the durations reported can't be
        // negative.
        let std_duration =
            |d: zx::MonotonicDuration| Duration::from_nanos(d.into_nanos().try_into().unwrap());
        match state {
            State::Started => {
                debug!("deferred resource removal: {info:?}")
            }
            State::Pending(d) => {
                warn!("resource removal pending for {:?}: {info:?}", std_duration(d))
            }
            State::Stuck(d) => {
                error!("resource removal stuck for {:?}: {info:?}", std_duration(d))
            }
            State::Finished(d) => {
                // Log at info if we possibly logged at warn or error to provide
                // closure.
                if d >= ACTION_INTERVAL {
                    info!("deferred resource removal complete in {:?}: {info:?}", std_duration(d))
                } else {
                    debug!("deferred resource removal complete in {:?}: {info:?}", std_duration(d))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::Poll;

    use assert_matches::assert_matches;
    use futures::channel::oneshot;
    use netstack3_core::sync::PrimaryRc;

    #[derive(Debug, Default)]
    struct ResourceA;

    #[derive(Debug, Default)]
    struct ResourceB;

    #[derive(Clone, Default)]
    struct SnoopingHandler {
        last_action: Rc<RefCell<Option<(State, ResourceItemInfo)>>>,
    }

    impl ActionHandler for SnoopingHandler {
        fn report_item(&self, state: State, info: &ResourceItemInfo) {
            assert_matches!(
                self.last_action.borrow_mut().replace((state, info.clone())),
                None,
                "unacknowledged action"
            )
        }
    }

    impl SnoopingHandler {
        #[track_caller]
        fn assert_action<T>(&self, expect: State) {
            let (state, ResourceItemInfo { typename, .. }) =
                self.last_action.borrow_mut().take().unwrap();
            assert_eq!(state, expect);
            assert_eq!(typename, std::any::type_name::<T>());
        }

        #[track_caller]
        fn assert_no_action(&self) {
            assert_matches!(self.last_action.borrow_mut().take(), None);
        }
    }

    fn new_deferred_resource<T: Default + Send + Sync + 'static>(
        sink: &ResourceRemovalSink,
    ) -> oneshot::Sender<T> {
        // Create a primary rc just to extract DebugReferences from it.
        let primary = PrimaryRc::new(T::default());
        let debug_references = PrimaryRc::debug_references(&primary).into_dyn();
        drop(primary);
        let (sender, receiver) = oneshot::channel();
        sink.defer_removal(
            debug_references,
            receiver.map(|r| r.expect("dropped sender")),
            |_: T| {},
        );
        sender
    }

    #[test]
    fn reports_actions() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let start = fasync::MonotonicInstant::now();
        let duration = || fasync::MonotonicInstant::now() - start;
        let handler = SnoopingHandler::default();
        let (worker, sink) = ResourceRemovalWorker::new();
        let fut = worker.run_with_handler(&handler);
        let mut fut = pin!(fut);

        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        handler.assert_no_action();
        let completer = new_deferred_resource::<ResourceA>(&sink);
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        handler.assert_action::<ResourceA>(State::Started);
        {
            // Interleave another resource to show we can handle both.
            let completer = new_deferred_resource::<ResourceB>(&sink);
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            handler.assert_action::<ResourceB>(State::Started);
            completer.send(Default::default()).unwrap();
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            handler.assert_action::<ResourceB>(State::Finished(duration()));
        }
        // Now observe all the Pending actions.
        for _ in 0..(STUCK_THRESHOLD - 1) {
            executor.set_fake_time(fasync::MonotonicInstant::now() + ACTION_INTERVAL);
            assert!(executor.wake_expired_timers());
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            handler.assert_action::<ResourceA>(State::Pending(duration()));
        }

        // Beyond the threshold, we observe the stuck state.
        for _ in 0..2 {
            executor.set_fake_time(fasync::MonotonicInstant::now() + ACTION_INTERVAL);
            assert!(executor.wake_expired_timers());
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            handler.assert_action::<ResourceA>(State::Stuck(duration()));
        }

        completer.send(Default::default()).unwrap();
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        handler.assert_action::<ResourceA>(State::Finished(duration()));
    }

    #[test]
    fn clean_shutdown() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let handler = SnoopingHandler::default();
        let (worker, sink) = ResourceRemovalWorker::new();
        let fut = worker.run_with_handler(&handler);
        let mut fut = pin!(fut);

        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        sink.close();
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[test]
    fn shutdown_waits_for_pending() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let handler = SnoopingHandler::default();
        let (worker, sink) = ResourceRemovalWorker::new();
        let fut = worker.run_with_handler(&handler);
        let mut fut = pin!(fut);

        let completer = new_deferred_resource::<ResourceA>(&sink);
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        handler.assert_action::<ResourceA>(State::Started);
        sink.close();

        // Still pending because we there's a resource waiting for removal.
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        completer.send(Default::default()).unwrap();
        // Now the worker shuts down.
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));
        handler.assert_action::<ResourceA>(State::Finished(zx::MonotonicDuration::ZERO));
    }

    #[test]
    #[should_panic(expected = "resource removal worker closed with stuck pending resource")]
    fn shutdown_panics_if_pending() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let start = fasync::MonotonicInstant::now();
        let duration = || fasync::MonotonicInstant::now() - start;
        let handler = SnoopingHandler::default();
        let (worker, sink) = ResourceRemovalWorker::new();
        let fut = worker.run_with_handler(&handler);
        let mut fut = pin!(fut);

        let completer = new_deferred_resource::<ResourceA>(&sink);
        assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        handler.assert_action::<ResourceA>(State::Started);

        // Close the sink, signalling the worker to shutdown.
        sink.close();

        // Now observe all the Pending actions.
        for _ in 0..(STUCK_THRESHOLD - 1) {
            executor.set_fake_time(fasync::MonotonicInstant::now() + ACTION_INTERVAL);
            assert!(executor.wake_expired_timers());
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            handler.assert_action::<ResourceA>(State::Pending(duration()));
        }

        // Panic here after we go over the interval threshold.
        executor.set_fake_time(fasync::MonotonicInstant::now() + ACTION_INTERVAL);
        assert!(executor.wake_expired_timers());
        let _ = executor.run_until_stalled(&mut fut);
        drop(completer);
    }

    #[test]
    #[should_panic(expected = "worker not running")]
    fn sink_panics_after_closing() {
        let (_worker, sink) = ResourceRemovalWorker::new();
        sink.close();
        // Panic here when we try to use the sink after closing.
        let _completer = new_deferred_resource::<ResourceA>(&sink);
    }
}
