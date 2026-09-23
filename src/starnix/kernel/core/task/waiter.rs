// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{CurrentTask, RunState};
use crate::vfs::{EpollEventHandler, FdNumber};
use bitflags::bitflags;
use futures::stream::AbortHandle;
use slab::Slab;
use smallvec::SmallVec;
use starnix_lifecycle::AtomicCounter;
use starnix_sync::{
    EventHandlerReadyQueueLock, EventWaitGuard, InterruptibleEvent, LockDepMutex, NotifyKind,
    PortEvent, PortWaitResult, PortWaiterCallbacksLock, PortWaiterWaitQueuesLock,
    WaitQueueImplLock, WaiterEventHandlerLock,
};
use starnix_types::ownership::debug_assert_no_local_temp_ref;
use starnix_uapi::error;
use starnix_uapi::errors::{EINTR, Errno};
use starnix_uapi::signals::{SIGKILL, SigSet, Signal};
use starnix_uapi::vfs::FdEvents;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Weak};
use syncio::zxio::zxio_signals_t;
use syncio::{ZxioSignals, ZxioWeak};

#[derive(Debug, Copy, Clone, Eq, Hash, PartialEq)]
pub enum ReadyItemKey {
    FdNumber(FdNumber),
    Usize(usize),
}

impl From<FdNumber> for ReadyItemKey {
    fn from(v: FdNumber) -> Self {
        Self::FdNumber(v)
    }
}

impl From<usize> for ReadyItemKey {
    fn from(v: usize) -> Self {
        Self::Usize(v)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ReadyItem {
    pub key: ReadyItemKey,
    pub events: FdEvents,
}

#[derive(Clone)]
pub enum EventHandler {
    /// Does nothing.
    ///
    /// It is up to the waiter to synchronize itself with the notifier if
    /// synchronization is needed.
    None,

    /// Enqueues an event to a ready list.
    ///
    /// This event handler naturally synchronizes the notifier and notifee
    /// because of the lock acquired/released when enqueuing the event.
    Enqueue {
        key: ReadyItemKey,
        queue: Arc<LockDepMutex<VecDeque<ReadyItem>, EventHandlerReadyQueueLock>>,
        sought_events: FdEvents,
    },

    /// Wraps another EventHandler and only triggers it once. Further .handle() calls are ignored.
    ///
    /// This is intended for cases like BinderFileObject which need to register
    /// the same EventHandler on multiple wait queues.
    HandleOnce(Arc<LockDepMutex<Option<EventHandler>, WaiterEventHandlerLock>>),

    /// This handler is an epoll.
    Epoll(EpollEventHandler),
}

impl EventHandler {
    pub fn handle(self, events: FdEvents) {
        match self {
            Self::None => {}
            Self::Enqueue { key, queue, sought_events } => {
                let events = events & sought_events;
                queue.lock().push_back(ReadyItem { key, events });
            }
            Self::HandleOnce(inner) => {
                if let Some(inner) = inner.lock().take() {
                    inner.handle(events);
                }
            }
            Self::Epoll(e) => e.handle(events),
        }
    }
}

pub struct ZxioSignalHandler {
    pub zxio: ZxioWeak,
    pub get_events_from_zxio_signals: fn(zxio_signals_t) -> FdEvents,
}

// The counter is incremented as each handle is signaled; when the counter reaches the handle
// count, the event handler is called with the given events.
pub struct ManyZxHandleSignalHandler {
    pub count: usize,
    pub counter: Arc<AtomicCounter<usize>>,
    pub expected_signals: zx::Signals,
    pub events: FdEvents,
}

pub enum SignalHandlerInner {
    None,
    Zxio(ZxioSignalHandler),
    ZxHandle(fn(zx::Signals) -> FdEvents),
    ManyZxHandle(ManyZxHandleSignalHandler),
}

pub struct SignalHandler {
    pub inner: SignalHandlerInner,
    pub event_handler: EventHandler,
    pub err_code: Option<Errno>,
}

impl SignalHandler {
    fn handle(self, signals: zx::Signals) -> Option<Errno> {
        let SignalHandler { inner, event_handler, err_code } = self;
        let events = match inner {
            SignalHandlerInner::None => None,
            SignalHandlerInner::Zxio(ZxioSignalHandler { zxio, get_events_from_zxio_signals }) => {
                if let Some(zxio) = zxio.upgrade() {
                    Some(get_events_from_zxio_signals(zxio.wait_end(signals)))
                } else {
                    None
                }
            }
            SignalHandlerInner::ZxHandle(get_events_from_zx_signals) => {
                Some(get_events_from_zx_signals(signals))
            }
            SignalHandlerInner::ManyZxHandle(signal_handler) => {
                if signals.contains(signal_handler.expected_signals) {
                    let new_count = signal_handler.counter.next() + 1;
                    assert!(new_count <= signal_handler.count);
                    if new_count == signal_handler.count {
                        Some(signal_handler.events)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };
        if let Some(events) = events {
            event_handler.handle(events)
        }
        err_code
    }
}

pub enum WaitCallback {
    SignalHandler(SignalHandler),
    EventHandler(EventHandler),
}

struct WaitCancelerQueue {
    wait_queue: Weak<LockDepMutex<WaitQueueImpl, WaitQueueImplLock>>,
    waiter: WaiterRef,
    wait_key: WaitKey,
    waiter_id: WaitEntryId,
}

struct WaitCancelerZxio {
    zxio: ZxioWeak,
    inner: PortWaitCanceler,
}

struct WaitCancelerPort {
    inner: PortWaitCanceler,
}

enum WaitCancelerInner {
    Zxio(WaitCancelerZxio),
    Queue(WaitCancelerQueue),
    Port(WaitCancelerPort),
}

enum NotifiableRef {
    Port(Arc<PortWaiter>),
    Event(Arc<InterruptibleEvent>),
    AbortHandle(Arc<AbortHandle>),
}

const WAIT_CANCELER_COMMON_SIZE: usize = 2;

/// Return values for wait_async methods.
///
/// Calling `cancel` will cancel any running wait.
///
/// Does not implement `Clone` or `Copy` so that only a single canceler exists
/// per wait.
pub struct WaitCanceler {
    cancellers: smallvec::SmallVec<[WaitCancelerInner; WAIT_CANCELER_COMMON_SIZE]>,
}

impl WaitCanceler {
    fn new_inner(inner: WaitCancelerInner) -> Self {
        Self { cancellers: smallvec::smallvec![inner] }
    }

    pub fn new_noop() -> Self {
        Self { cancellers: Default::default() }
    }

    pub fn new_zxio(zxio: ZxioWeak, inner: PortWaitCanceler) -> Self {
        Self::new_inner(WaitCancelerInner::Zxio(WaitCancelerZxio { zxio, inner }))
    }

    pub fn new_port(inner: PortWaitCanceler) -> Self {
        Self::new_inner(WaitCancelerInner::Port(WaitCancelerPort { inner }))
    }

    /// Equivalent to `merge_unbounded`, except that it enforces that the resulting vector of
    /// cancellers is small enough to avoid being separately allocated on the heap.
    ///
    /// If possible, use this function instead of `merge_unbounded`, because it gives us better
    /// tools to keep this code path optimized.
    pub fn merge(self, other: Self) -> Self {
        // Increase `WAIT_CANCELER_COMMON_SIZE` if needed, or remove this assert and allow the
        // smallvec to allocate.
        assert!(
            self.cancellers.len() + other.cancellers.len() <= WAIT_CANCELER_COMMON_SIZE,
            "WaitCanceler::merge disallows more than {} cancellers, found {} + {}",
            WAIT_CANCELER_COMMON_SIZE,
            self.cancellers.len(),
            other.cancellers.len()
        );
        WaitCanceler::merge_unbounded(self, other)
    }

    /// Creates a new `WaitCanceler` that is equivalent to canceling both its arguments.
    pub fn merge_unbounded(
        Self { mut cancellers }: Self,
        Self { cancellers: mut other }: Self,
    ) -> Self {
        cancellers.append(&mut other);
        WaitCanceler { cancellers }
    }

    /// Cancel the pending wait.
    ///
    /// Takes `self` by value since a wait can only be canceled once.
    pub fn cancel(self) {
        let Self { cancellers } = self;
        for canceller in cancellers.into_iter().rev() {
            match canceller {
                WaitCancelerInner::Zxio(WaitCancelerZxio { zxio, inner }) => {
                    let Some(zxio) = zxio.upgrade() else { return };
                    let (_, signals) = zxio.wait_begin(ZxioSignals::NONE.bits());
                    inner.cancel();
                    zxio.wait_end(signals);
                }
                WaitCancelerInner::Queue(WaitCancelerQueue {
                    wait_queue,
                    waiter,
                    wait_key,
                    waiter_id: WaitEntryId { key, id },
                }) => {
                    let Some(wait_queue) = wait_queue.upgrade() else { return };
                    waiter.remove_callback(&wait_key);
                    waiter.will_remove_from_wait_queue(&wait_key);
                    let mut wait_queue = wait_queue.lock();
                    let waiters = &mut wait_queue.waiters;
                    if let Some(entry) = waiters.get_mut(key) {
                        // The map of waiters in a wait queue uses a `Slab` which
                        // recycles keys. To make sure we are removing the right
                        // entry, make sure the ID value matches what we expect
                        // to remove.
                        if entry.id == id {
                            waiters.remove(key);
                        }
                    }
                }
                WaitCancelerInner::Port(WaitCancelerPort { inner }) => {
                    inner.cancel();
                }
            }
        }
    }
}

/// Return values for wait_async methods that monitor the state of a handle.
///
/// Calling `cancel` will cancel any running wait.
///
/// Does not implement `Clone` or `Copy` so that only a single canceler exists
/// per wait.
pub struct PortWaitCanceler {
    waiter: Weak<PortWaiter>,
    key: WaitKey,
}

impl PortWaitCanceler {
    /// Cancel the pending wait.
    ///
    /// Takes `self` by value since a wait can only be canceled once.
    pub fn cancel(self) {
        let Self { waiter, key } = self;
        if let Some(waiter) = waiter.upgrade() {
            let _ = waiter.port.cancel(key.raw);
            waiter.remove_callback(&key);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct WaitKey {
    raw: u64,
}

/// The different type of event that can be waited on / triggered.
#[derive(Clone, Copy, Debug)]
enum WaitEvents {
    /// All event: a wait on `All` will be woken up by all event, and a trigger on `All` will wake
    /// every waiter.
    All,
    /// Wait on the set of FdEvents.
    Fd(FdEvents),
    /// Wait for the specified value.
    Value(u64),
    /// Wait for a signal in a specific mask to be received by the task.
    SignalMask(SigSet),
}

impl WaitEvents {
    /// Returns whether a wait on `self` should be woken up by `other`.
    fn intercept(self: &WaitEvents, other: &WaitEvents) -> bool {
        match (self, other) {
            (Self::All, _) | (_, Self::All) => true,
            (Self::Fd(m1), Self::Fd(m2)) => m1.bits() & m2.bits() != 0,
            (Self::Value(v1), Self::Value(v2)) => v1 == v2,
            // A SignalMask event can only be intercepted by another SignalMask event.
            (Self::SignalMask(m1), Self::SignalMask(m2)) => m1.intersects(m2),
            _ => false,
        }
    }
}

impl WaitCallback {
    pub fn none() -> EventHandler {
        EventHandler::None
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct WaiterOptions: u8 {
        /// The wait cannot be interrupted by signals.
        const IGNORE_SIGNALS   = 1 << 0;

        /// The wait is not taking place at a safe point.
        ///
        /// For example, the caller might be holding a lock, which could cause a deadlock if the
        /// waiter triggers delayed releasers.
        const UNSAFE_CALLSTACK = 1 << 1;
    }
}

/// Implementation of Waiter. We put the Waiter data in an Arc so that WaitQueue can tell when the
/// Waiter has been destroyed by keeping a Weak reference. But this is an implementation detail and
/// a Waiter should have a single owner. So the Arc is hidden inside Waiter.
struct PortWaiter {
    port: PortEvent,
    callbacks: LockDepMutex<HashMap<WaitKey, WaitCallback>, PortWaiterCallbacksLock>, // the key 0 is reserved for 'no handler'
    next_key: AtomicCounter<u64>,
    options: WaiterOptions,

    /// Collection of wait queues this Waiter is waiting on, so that when the Waiter is Dropped it
    /// can remove itself from the queues.
    ///
    /// This lock is nested inside the WaitQueue.waiters lock.
    wait_queues: LockDepMutex<
        HashMap<WaitKey, Weak<LockDepMutex<WaitQueueImpl, WaitQueueImplLock>>>,
        PortWaiterWaitQueuesLock,
    >,
}

impl PortWaiter {
    /// Internal constructor.
    fn new(options: WaiterOptions) -> Arc<Self> {
        Arc::new(PortWaiter {
            port: PortEvent::new(),
            callbacks: Default::default(),
            next_key: AtomicCounter::<u64>::new(1),
            options,
            wait_queues: Default::default(),
        })
    }

    /// Waits until the given deadline has passed or the waiter is woken up. See wait_until().
    fn wait_internal(&self, deadline: zx::MonotonicInstant) -> Result<(), Errno> {
        // This method can block arbitrarily long, possibly waiting for another process. The
        // current thread should not own any local ref that might delay the release of a resource
        // while doing so.
        debug_assert_no_local_temp_ref();

        match self.port.wait(deadline) {
            PortWaitResult::Notification { kind: NotifyKind::Regular } => Ok(()),
            PortWaitResult::Notification { kind: NotifyKind::Interrupt } => error!(EINTR),
            PortWaitResult::Signal { key, observed } => {
                if let Some(callback) = self.remove_callback(&WaitKey { raw: key }) {
                    match callback {
                        WaitCallback::SignalHandler(handler) => {
                            if let Some(errno) = handler.handle(observed) {
                                return Err(errno);
                            }
                        }
                        WaitCallback::EventHandler(_) => {
                            panic!("wrong type of handler called")
                        }
                    }
                }

                Ok(())
            }
            PortWaitResult::TimedOut => error!(ETIMEDOUT),
        }
    }

    fn wait_until(
        self: &Arc<Self>,
        current_task: &CurrentTask,
        run_state: RunState,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        let is_waiting = deadline.into_nanos() > 0;

        let callback = || {
            // We are susceptible to spurious wakeups because interrupt() posts a message to the port
            // queue. In addition to more subtle races, there could already be valid messages in the
            // port queue that will immediately wake us up, leaving the interrupt message in the queue
            // for subsequent waits (which by then may not have any signals pending) to read.
            //
            // It's impossible to non-racily guarantee that a signal is pending so there might always
            // be an EINTR result here with no signal. But any signal we get when !is_waiting we know is
            // leftover from before: the top of this function only sets ourself as the
            // current_task.signals.run_state when there's a nonzero timeout, and that waiter reference
            // is what is used to signal the interrupt().
            loop {
                let wait_result = self.wait_internal(deadline);
                if let Err(errno) = &wait_result {
                    if errno.code == EINTR && !is_waiting {
                        continue; // Spurious wakeup.
                    }
                }
                return wait_result;
            }
        };

        // Trigger delayed releaser before blocking if we're at a safe point.
        //
        // For example, we cannot trigger delayed releaser if we are holding any locks.
        if !self.options.contains(WaiterOptions::UNSAFE_CALLSTACK) {
            current_task.trigger_delayed_releaser();
        }

        if is_waiting { current_task.run_in_state(run_state, callback) } else { callback() }
    }

    fn next_key(&self) -> WaitKey {
        let key = self.next_key.next();
        // TODO - find a better reaction to wraparound
        assert!(key != 0, "bad key from u64 wraparound");
        WaitKey { raw: key }
    }

    fn register_callback(&self, callback: WaitCallback) -> WaitKey {
        let key = self.next_key();
        assert!(
            self.callbacks.lock().insert(key, callback).is_none(),
            "unexpected callback already present for key {key:?}"
        );
        key
    }

    fn remove_callback(&self, key: &WaitKey) -> Option<WaitCallback> {
        self.callbacks.lock().remove(&key)
    }

    fn wake_immediately(&self, events: FdEvents, handler: EventHandler) {
        let callback = WaitCallback::EventHandler(handler);
        let key = self.register_callback(callback);
        self.queue_events(&key, WaitEvents::Fd(events));
    }

    /// Establish an asynchronous wait for the signals on the given Zircon handle (not to be
    /// confused with POSIX signals), optionally running a FnOnce. Wait operations will return
    /// the error code present in the provided SignalHandler.
    ///
    /// Returns a `PortWaitCanceler` that can be used to cancel the wait.
    fn wake_on_zircon_signals(
        self: &Arc<Self>,
        handle: &dyn zx::AsHandleRef,
        zx_signals: zx::Signals,
        handler: SignalHandler,
    ) -> Result<PortWaitCanceler, zx::Status> {
        let callback = WaitCallback::SignalHandler(handler);
        let key = self.register_callback(callback);
        self.port.object_wait_async(
            handle,
            key.raw,
            zx_signals,
            zx::WaitAsyncOpts::EDGE_TRIGGERED,
        )?;
        Ok(PortWaitCanceler { waiter: Arc::downgrade(self), key })
    }

    fn queue_events(&self, key: &WaitKey, events: WaitEvents) {
        scopeguard::defer! {
            self.port.notify(NotifyKind::Regular)
        }

        // Handling user events immediately when they are triggered breaks any
        // ordering expectations on Linux by batching all starnix events with
        // the first starnix event even if other events occur on the Fuchsia
        // platform (and are enqueued to the `zx::Port`) between them. This
        // ordering does not seem to be load-bearing for applications running on
        // starnix so we take the divergence in ordering in favour of improved
        // performance (by minimizing syscalls) when operating on FDs backed by
        // starnix.
        //
        // TODO(https://fxbug.dev/42084319): If we can read a batch of packets
        // from the `zx::Port`, maybe we can keep the ordering?
        let Some(callback) = self.remove_callback(key) else {
            return;
        };

        match callback {
            WaitCallback::EventHandler(handler) => {
                let events = match events {
                    // If the event is All, signal on all possible fd
                    // events.
                    WaitEvents::All => FdEvents::all(),
                    WaitEvents::Fd(events) => events,
                    WaitEvents::SignalMask(_) => FdEvents::POLLIN,
                    WaitEvents::Value(_) => FdEvents::POLLIN,
                };
                handler.handle(events)
            }
            WaitCallback::SignalHandler(_) => {
                panic!("wrong type of handler called")
            }
        }
    }

    fn notify(&self) {
        self.port.notify(NotifyKind::Regular);
    }

    fn interrupt(&self) {
        if self.options.contains(WaiterOptions::IGNORE_SIGNALS) {
            return;
        }
        self.port.notify(NotifyKind::Interrupt);
    }
}

impl std::fmt::Debug for PortWaiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortWaiter").field("port", &self.port).finish_non_exhaustive()
    }
}

/// A type that can put a thread to sleep waiting for a condition.
#[derive(Debug, Clone)]
pub struct Waiter {
    // TODO(https://g-issues.fuchsia.dev/issues/303068424): Avoid `PortWaiter`
    // when operating purely over FDs backed by starnix.
    inner: Arc<PortWaiter>,
}

impl Waiter {
    /// Create a new waiter.
    pub fn new() -> Self {
        Self { inner: PortWaiter::new(WaiterOptions::empty()) }
    }

    /// Create a new waiter with the given options.
    pub fn with_options(options: WaiterOptions) -> Self {
        Self { inner: PortWaiter::new(options) }
    }

    /// Create a weak reference to this waiter.
    fn weak(&self) -> WaiterRef {
        WaiterRef::from_port(&self.inner)
    }

    /// Freeze the task until the waiter is woken up.
    ///
    /// No signal, e.g. EINTR (interrupt), should be received.
    pub fn freeze(&self, current_task: &CurrentTask) {
        while self
            .inner
            .wait_until(
                current_task,
                RunState::Frozen(self.clone()),
                zx::MonotonicInstant::INFINITE,
            )
            .is_err()
        {
            // Avoid attempting to freeze the task if there is a pending SIGKILL.
            if current_task.read().has_signal_pending(SIGKILL) {
                break;
            }
            // Ignore spurious wakeups from the [`PortEvent.futex`]
        }
    }

    /// Wait until the waiter is woken up.
    ///
    /// If the wait is interrupted (see [`Waiter::interrupt`]), this function returns EINTR.
    pub fn wait(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        self.inner.wait_until(
            current_task,
            RunState::Waiter(WaiterRef::from_port(&self.inner)),
            zx::MonotonicInstant::INFINITE,
        )
    }

    /// Wait until the given deadline has passed or the waiter is woken up.
    ///
    /// If the wait deadline is nonzero and is interrupted (see [`Waiter::interrupt`]), this
    /// function returns EINTR. Callers must take special care not to lose any accumulated data or
    /// local state when EINTR is received as this is a normal and recoverable situation.
    ///
    /// Using a 0 deadline (no waiting, useful for draining pending events) will not wait and is
    /// guaranteed not to issue EINTR.
    ///
    /// It the timeout elapses with no events, this function returns ETIMEDOUT.
    ///
    /// Processes at most one event. If the caller is interested in draining the events, it should
    /// repeatedly call this function with a 0 deadline until it reports ETIMEDOUT. (This case is
    /// why a 0 deadline must not return EINTR, as previous calls to wait_until() may have
    /// accumulated state that would be lost when returning EINTR to userspace.)
    ///
    /// It is up to the caller (the "waiter") to make sure that it synchronizes with any object
    /// that triggers an event (the "notifier"). This `Waiter` does not provide any synchronization
    /// itself. Note that synchronization between the "waiter" the "notifier" may be provided by
    /// the [`EventHandler`] used to handle an event iff the waiter observes the side-effects of
    /// the handler (e.g. reading the ready list modified by [`EventHandler::Enqueue`] or
    /// [`EventHandler::EnqueueOnce`]).
    pub fn wait_until(
        &self,
        current_task: &CurrentTask,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        self.inner.wait_until(
            current_task,
            RunState::Waiter(WaiterRef::from_port(&self.inner)),
            deadline,
        )
    }

    fn create_wait_entry(&self, filter: WaitEvents) -> WaitEntry {
        WaitEntry { waiter: self.weak(), filter, key: self.inner.next_key() }
    }

    fn create_wait_entry_with_handler(
        &self,
        filter: WaitEvents,
        handler: EventHandler,
    ) -> WaitEntry {
        let key = self.inner.register_callback(WaitCallback::EventHandler(handler));
        WaitEntry { waiter: self.weak(), filter, key }
    }

    pub fn wake_immediately(&self, events: FdEvents, handler: EventHandler) {
        self.inner.wake_immediately(events, handler);
    }

    /// Establish an asynchronous wait for the signals on the given Zircon handle (not to be
    /// confused with POSIX signals), optionally running a FnOnce.
    ///
    /// Returns a `PortWaitCanceler` that can be used to cancel the wait.
    pub fn wake_on_zircon_signals(
        &self,
        handle: &dyn zx::AsHandleRef,
        zx_signals: zx::Signals,
        handler: SignalHandler,
    ) -> Result<PortWaitCanceler, zx::Status> {
        self.inner.wake_on_zircon_signals(handle, zx_signals, handler)
    }

    /// Return a WaitCanceler representing a wait that will never complete. Useful for stub
    /// implementations that should block forever even though a real implementation would wake up
    /// eventually.
    pub fn fake_wait(&self) -> WaitCanceler {
        WaitCanceler::new_noop()
    }

    // Notify the waiter to wake it up without signalling any events.
    pub fn notify(&self) {
        self.inner.notify();
    }

    /// Interrupt the waiter to deliver a signal. The wait operation will return EINTR, and a
    /// typical caller should then unwind to the syscall dispatch loop to let the signal be
    /// processed. See wait_until() for more details.
    ///
    /// Ignored if the waiter was created with new_ignoring_signals().
    pub fn interrupt(&self) {
        self.inner.interrupt();
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        // Delete ourselves from each wait queue we know we're on to prevent Weak references to
        // ourself from sticking around forever.
        let wait_queues = std::mem::take(&mut *self.inner.wait_queues.lock()).into_values();
        for wait_queue in wait_queues {
            if let Some(wait_queue) = wait_queue.upgrade() {
                wait_queue.lock().waiters.retain(|_, entry| entry.entry.waiter != *self)
            }
        }
    }
}

impl Default for Waiter {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Waiter {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

pub struct SimpleWaiter {
    event: Arc<InterruptibleEvent>,
    wait_queues: Vec<Weak<LockDepMutex<WaitQueueImpl, WaitQueueImplLock>>>,
}

impl SimpleWaiter {
    pub fn new(event: &Arc<InterruptibleEvent>) -> (SimpleWaiter, EventWaitGuard<'_>) {
        (SimpleWaiter { event: event.clone(), wait_queues: Default::default() }, event.begin_wait())
    }
}

impl Drop for SimpleWaiter {
    fn drop(&mut self) {
        for wait_queue in &self.wait_queues {
            if let Some(wait_queue) = wait_queue.upgrade() {
                wait_queue.lock().waiters.retain(|_, entry| entry.entry.waiter != self.event)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum WaiterKind {
    Port(Weak<PortWaiter>),
    Event(Weak<InterruptibleEvent>),
    AbortHandle(Weak<futures::stream::AbortHandle>),
}

impl Default for WaiterKind {
    fn default() -> Self {
        WaiterKind::Port(Default::default())
    }
}

/// A weak reference to a Waiter. Intended for holding in wait queues or stashing elsewhere for
/// calling queue_events later.
#[derive(Debug, Default, Clone)]
pub struct WaiterRef(WaiterKind);

impl WaiterRef {
    fn from_port(waiter: &Arc<PortWaiter>) -> WaiterRef {
        WaiterRef(WaiterKind::Port(Arc::downgrade(waiter)))
    }

    fn from_event(event: &Arc<InterruptibleEvent>) -> WaiterRef {
        WaiterRef(WaiterKind::Event(Arc::downgrade(event)))
    }

    pub fn from_abort_handle(handle: &Arc<futures::stream::AbortHandle>) -> WaiterRef {
        WaiterRef(WaiterKind::AbortHandle(Arc::downgrade(handle)))
    }

    pub fn is_valid(&self) -> bool {
        match &self.0 {
            WaiterKind::Port(waiter) => waiter.strong_count() != 0,
            WaiterKind::Event(event) => event.strong_count() != 0,
            WaiterKind::AbortHandle(handle) => handle.strong_count() != 0,
        }
    }

    pub fn interrupt(&self) {
        match &self.0 {
            WaiterKind::Port(waiter) => {
                if let Some(waiter) = waiter.upgrade() {
                    waiter.interrupt();
                }
            }
            WaiterKind::Event(event) => {
                if let Some(event) = event.upgrade() {
                    event.interrupt();
                }
            }
            WaiterKind::AbortHandle(handle) => {
                if let Some(handle) = handle.upgrade() {
                    handle.abort();
                }
            }
        }
    }

    fn remove_callback(&self, key: &WaitKey) {
        match &self.0 {
            WaiterKind::Port(waiter) => {
                if let Some(waiter) = waiter.upgrade() {
                    waiter.remove_callback(key);
                }
            }
            _ => (),
        }
    }

    /// Attempts to upgrade a waiter ref to a notifiable ref. If the waiter ref is no
    /// longer valid, returns None.
    fn upgrade_notifiable(&self) -> Option<NotifiableRef> {
        match &self.0 {
            WaiterKind::Port(waiter) => {
                if let Some(waiter) = waiter.upgrade() {
                    return Some(NotifiableRef::Port(waiter));
                }
            }
            WaiterKind::Event(event) => {
                if let Some(event) = event.upgrade() {
                    return Some(NotifiableRef::Event(event));
                }
            }
            WaiterKind::AbortHandle(handle) => {
                if let Some(handle) = handle.upgrade() {
                    return Some(NotifiableRef::AbortHandle(handle));
                }
            }
        }
        None
    }

    /// Called by the WaitQueue when this waiter is about to be removed from the queue.
    ///
    /// TODO(abarth): This function does not appear to be called when the WaitQueue is dropped,
    /// which appears to be a leak.
    fn will_remove_from_wait_queue(&self, key: &WaitKey) {
        match &self.0 {
            WaiterKind::Port(waiter) => {
                if let Some(waiter) = waiter.upgrade() {
                    waiter.wait_queues.lock().remove(key);
                }
            }
            _ => (),
        }
    }
}

impl PartialEq<Waiter> for WaiterRef {
    fn eq(&self, other: &Waiter) -> bool {
        match &self.0 {
            WaiterKind::Port(waiter) => waiter.as_ptr() == Arc::as_ptr(&other.inner),
            _ => false,
        }
    }
}

impl PartialEq<Arc<InterruptibleEvent>> for WaiterRef {
    fn eq(&self, other: &Arc<InterruptibleEvent>) -> bool {
        match &self.0 {
            WaiterKind::Event(event) => event.as_ptr() == Arc::as_ptr(other),
            _ => false,
        }
    }
}

impl PartialEq for WaiterRef {
    fn eq(&self, other: &WaiterRef) -> bool {
        match (&self.0, &other.0) {
            (WaiterKind::Port(lhs), WaiterKind::Port(rhs)) => Weak::ptr_eq(lhs, rhs),
            (WaiterKind::Event(lhs), WaiterKind::Event(rhs)) => Weak::ptr_eq(lhs, rhs),
            (WaiterKind::AbortHandle(lhs), WaiterKind::AbortHandle(rhs)) => Weak::ptr_eq(lhs, rhs),
            _ => false,
        }
    }
}

impl NotifiableRef {
    fn notify(&self, key: &WaitKey, events: WaitEvents) {
        match self {
            NotifiableRef::Port(port_waiter) => port_waiter.queue_events(key, events),
            NotifiableRef::Event(interruptible_event) => interruptible_event.notify(),
            NotifiableRef::AbortHandle(handle) => handle.abort(),
        }
    }
}

/// A list of waiters waiting for some event.
///
/// For events that are generated inside Starnix, we walk the wait queue
/// on the thread that triggered the event to notify the waiters that the event
/// has occurred. The waiters will then wake up on their own thread to handle
/// the event.
#[derive(Default, Debug, Clone)]
pub struct WaitQueue(Arc<LockDepMutex<WaitQueueImpl, WaitQueueImplLock>>);

impl WaitQueue {
    pub fn new() -> Self {
        Self(Arc::new(WaitQueueImpl::default().into()))
    }
}

#[derive(Debug)]
struct WaitEntryWithId {
    entry: WaitEntry,
    /// The ID use to uniquely identify this wait entry even if it shares the
    /// key used in the wait queue's [`Slab`] with another wait entry since a
    /// slab's keys are recycled.
    id: u64,
}

struct WaitEntryId {
    key: usize,
    id: u64,
}

#[derive(Default, Debug)]
struct WaitQueueImpl {
    /// Holds the next ID value to use when adding a new `WaitEntry` to the
    /// waiters (dense) map.
    ///
    /// A [`Slab`]s keys are recycled so we use the ID to uniquely identify a
    /// wait entry.
    next_wait_entry_id: u64,
    /// The list of waiters.
    ///
    /// The waiter's wait_queues lock is nested inside this lock.
    waiters: Slab<WaitEntryWithId>,
}

/// An entry in a WaitQueue.
#[derive(Debug)]
struct WaitEntry {
    /// The waiter that is waking for the FdEvent.
    waiter: WaiterRef,

    /// The events that the waiter is waiting for.
    filter: WaitEvents,

    /// key for cancelling and queueing events
    key: WaitKey,
}

impl WaitQueue {
    fn add_waiter(&self, entry: WaitEntry) -> WaitEntryId {
        let mut wait_queue = self.0.lock();
        let id = wait_queue
            .next_wait_entry_id
            .checked_add(1)
            .expect("all possible wait entry ID values exhausted");
        wait_queue.next_wait_entry_id = id;
        WaitEntryId { key: wait_queue.waiters.insert(WaitEntryWithId { entry, id }), id }
    }

    /// Establish a wait for the given entry.
    ///
    /// The waiter will be notified when an event matching the entry occurs.
    ///
    /// This function does not actually block the waiter. To block the waiter,
    /// call the [`Waiter::wait`] function on the waiter.
    ///
    /// Returns a `WaitCanceler` that can be used to cancel the wait.
    fn wait_async_entry(&self, waiter: &Waiter, entry: WaitEntry) -> WaitCanceler {
        let wait_key = entry.key;
        let waiter_id = self.add_waiter(entry);
        let wait_queue = Arc::downgrade(&self.0);
        waiter.inner.wait_queues.lock().insert(wait_key, wait_queue.clone());
        WaitCanceler::new_inner(WaitCancelerInner::Queue(WaitCancelerQueue {
            wait_queue,
            waiter: waiter.weak(),
            wait_key,
            waiter_id,
        }))
    }

    /// Establish a wait for the given value event.
    ///
    /// The waiter will be notified when an event with the same value occurs.
    ///
    /// This function does not actually block the waiter. To block the waiter,
    /// call the [`Waiter::wait`] function on the waiter.
    ///
    /// Returns a `WaitCanceler` that can be used to cancel the wait.
    pub fn wait_async_value(&self, waiter: &Waiter, value: u64) -> WaitCanceler {
        self.wait_async_entry(waiter, waiter.create_wait_entry(WaitEvents::Value(value)))
    }

    /// Establish a wait for the given value event with an associated handler.
    ///
    /// The waiter will be notified when an event with the same value occurs, triggering the handler.
    ///
    /// This function does not actually block the waiter. To block the waiter,
    /// call the [`Waiter::wait`] function on the waiter.
    ///
    /// Returns a `WaitCanceler` that can be used to cancel the wait.
    pub fn wait_async_value_with_handler(
        &self,
        waiter: &Waiter,
        value: u64,
        handler: EventHandler,
    ) -> WaitCanceler {
        let entry = waiter.create_wait_entry_with_handler(WaitEvents::Value(value), handler);
        self.wait_async_entry(waiter, entry)
    }

    /// Establish a wait for the given FdEvents.
    ///
    /// The waiter will be notified when an event matching the `events` occurs.
    ///
    /// This function does not actually block the waiter. To block the waiter,
    /// call the [`Waiter::wait`] function on the waiter.
    ///
    /// Returns a `WaitCanceler` that can be used to cancel the wait.
    pub fn wait_async_fd_events(
        &self,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        let entry = waiter.create_wait_entry_with_handler(WaitEvents::Fd(events), handler);
        self.wait_async_entry(waiter, entry)
    }

    /// Establish a wait for a particular signal mask.
    ///
    /// The waiter will be notified when a signal in the mask is received.
    ///
    /// This function does not actually block the waiter. To block the waiter,
    /// call the [`Waiter::wait`] function on the waiter.
    ///
    /// Returns a `WaitCanceler` that can be used to cancel the wait.
    pub fn wait_async_signal_mask(
        &self,
        waiter: &Waiter,
        mask: SigSet,
        handler: EventHandler,
    ) -> WaitCanceler {
        let entry = waiter.create_wait_entry_with_handler(WaitEvents::SignalMask(mask), handler);
        self.wait_async_entry(waiter, entry)
    }

    /// Establish a wait for any event.
    ///
    /// The waiter will be notified when any event occurs.
    ///
    /// This function does not actually block the waiter. To block the waiter,
    /// call the [`Waiter::wait`] function on the waiter.
    ///
    /// Returns a `WaitCanceler` that can be used to cancel the wait.
    pub fn wait_async(&self, waiter: &Waiter) -> WaitCanceler {
        self.wait_async_entry(waiter, waiter.create_wait_entry(WaitEvents::All))
    }

    pub fn wait_async_simple(&self, waiter: &mut SimpleWaiter) {
        let entry = WaitEntry {
            waiter: WaiterRef::from_event(&waiter.event),
            filter: WaitEvents::All,
            key: Default::default(),
        };
        waiter.wait_queues.push(Arc::downgrade(&self.0));
        self.add_waiter(entry);
    }

    fn notify_events_count(&self, mut events: WaitEvents, mut limit: usize) -> usize {
        if let WaitEvents::Fd(ref mut fd_events) = events {
            *fd_events = fd_events.add_equivalent_fd_events();
        }
        // Store references to waiters ready to be notified locally so that we can drop our waiters
        // lock before notifying the waiters. The waiters will need to acquire the waiters lock once
        // they wake up in order to remove themselves from the queue and so they might contend
        // with this thread for that lock.
        // Usually we expect to notify at most a single waiter.
        let mut notifiable_refs = SmallVec::<[(NotifiableRef, WaitKey); 1]>::new();
        let mut woken = 0;
        {
            let mut guard = self.0.lock();
            guard.waiters.retain(|_, WaitEntryWithId { entry, id: _ }| {
                if limit > 0 && entry.filter.intercept(&events) {
                    if let Some(notifiable_ref) = entry.waiter.upgrade_notifiable() {
                        limit -= 1;
                        woken += 1;
                        notifiable_refs.push((notifiable_ref, entry.key));
                    }

                    entry.waiter.will_remove_from_wait_queue(&entry.key);
                    false
                } else {
                    true
                }
            });
        }
        for (notifiable_ref, key) in notifiable_refs {
            notifiable_ref.notify(&key, events);
        }
        woken
    }

    pub fn notify_fd_events(&self, events: FdEvents) {
        self.notify_events_count(WaitEvents::Fd(events), usize::MAX);
    }

    pub fn notify_fd_events_count(&self, events: FdEvents, limit: usize) {
        self.notify_events_count(WaitEvents::Fd(events), limit);
    }

    pub fn notify_signal(&self, signal: &Signal) {
        let event = WaitEvents::SignalMask(SigSet::from(*signal));
        self.notify_events_count(event, usize::MAX);
    }

    pub fn notify_value(&self, value: u64) {
        self.notify_events_count(WaitEvents::Value(value), usize::MAX);
    }

    pub fn notify_unordered_count(&self, limit: usize) {
        self.notify_events_count(WaitEvents::All, limit);
    }

    pub fn notify_all(&self) {
        self.notify_unordered_count(usize::MAX);
    }

    /// Returns whether there is no active waiters waiting on this `WaitQueue`.
    pub fn is_empty(&self) -> bool {
        self.0.lock().waiters.is_empty()
    }
}

/// A wait queue that dispatches events based on the value of an enum.
pub struct TypedWaitQueue<T: Into<u64>> {
    wait_queue: WaitQueue,
    value_type: std::marker::PhantomData<T>,
}

// We can't #[derive(Default)] on [TypedWaitQueue<T>] as T may not implement the Default trait.
impl<T: Into<u64>> Default for TypedWaitQueue<T> {
    fn default() -> Self {
        Self { wait_queue: Default::default(), value_type: Default::default() }
    }
}

impl<T: Into<u64>> TypedWaitQueue<T> {
    pub fn wait_async_value(&self, waiter: &Waiter, value: T) -> WaitCanceler {
        self.wait_queue.wait_async_value(waiter, value.into())
    }

    pub fn wait_async_value_with_handler(
        &self,
        waiter: &Waiter,
        value: T,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.wait_queue.wait_async_value_with_handler(waiter, value.into(), handler)
    }

    pub fn notify_value(&self, value: T) {
        self.wait_queue.notify_value(value.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::fuchsia::create_fuchsia_pipe;
    use crate::signals::SignalInfo;
    use crate::task::TaskFlags;
    use crate::testing::{spawn_kernel_and_run, spawn_kernel_and_run_sync};
    use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
    use crate::vfs::eventfd::{EventFdType, new_eventfd};
    use assert_matches::assert_matches;
    use starnix_uapi::open_flags::OpenFlags;
    use starnix_uapi::signals::SIGUSR1;

    const KEY: ReadyItemKey = ReadyItemKey::Usize(1234);

    #[::fuchsia::test]
    async fn test_async_wait_exec() {
        spawn_kernel_and_run(async |current_task| {
            let (local_socket, remote_socket) = zx::Socket::create_stream();
            let pipe = create_fuchsia_pipe(&current_task, remote_socket, OpenFlags::RDWR).unwrap();

            const MEM_SIZE: usize = 1024;
            let mut output_buffer = VecOutputBuffer::new(MEM_SIZE);

            let test_string = "hello startnix".to_string();
            let queue: Arc<LockDepMutex<VecDeque<ReadyItem>, EventHandlerReadyQueueLock>> =
                Default::default();
            let handler = EventHandler::Enqueue {
                key: KEY,
                queue: queue.clone(),
                sought_events: FdEvents::all(),
            };
            let waiter = Waiter::new();
            pipe.wait_async(&current_task, &waiter, FdEvents::POLLIN, handler).expect("wait_async");
            let test_string_clone = test_string.clone();

            let write_count = AtomicCounter::<usize>::default();
            std::thread::scope(|s| {
                let thread = s.spawn(|| {
                    let test_data = test_string_clone.as_bytes();
                    let no_written = local_socket.write(test_data).unwrap();
                    assert_eq!(0, write_count.add(no_written));
                    assert_eq!(no_written, test_data.len());
                });

                // this code would block on failure

                assert!(queue.lock().is_empty());
                waiter.wait(&current_task).unwrap();
                thread.join().expect("join thread")
            });
            queue.lock().iter().for_each(|item| assert!(item.events.contains(FdEvents::POLLIN)));

            let read_size = pipe.read(&current_task, &mut output_buffer).unwrap();

            let no_written = write_count.get();
            assert_eq!(no_written, read_size);

            assert_eq!(output_buffer.data(), test_string.as_bytes());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_async_wait_cancel() {
        for do_cancel in [true, false] {
            spawn_kernel_and_run(async move |current_task| {
                let event = new_eventfd(&current_task, 0, EventFdType::Counter, true);
                let waiter = Waiter::new();
                let queue: Arc<LockDepMutex<VecDeque<ReadyItem>, EventHandlerReadyQueueLock>> =
                    Default::default();
                let handler = EventHandler::Enqueue {
                    key: KEY,
                    queue: queue.clone(),
                    sought_events: FdEvents::all(),
                };
                let wait_canceler = event
                    .wait_async(&current_task, &waiter, FdEvents::POLLIN, handler)
                    .expect("wait_async");
                if do_cancel {
                    wait_canceler.cancel();
                }
                let add_val = 1u64;
                assert_eq!(
                    event
                        .write(&current_task, &mut VecInputBuffer::new(&add_val.to_ne_bytes()))
                        .unwrap(),
                    std::mem::size_of::<u64>()
                );

                let wait_result = waiter.wait_until(&current_task, zx::MonotonicInstant::ZERO);
                let final_count = queue.lock().len();
                if do_cancel {
                    assert_eq!(wait_result, error!(ETIMEDOUT));
                    assert_eq!(0, final_count);
                } else {
                    assert_eq!(wait_result, Ok(()));
                    assert_eq!(1, final_count);
                }
            })
            .await;
        }
    }

    #[::fuchsia::test]
    async fn single_waiter_multiple_waits_cancel_one_waiter_still_notified() {
        spawn_kernel_and_run(async |current_task| {
            let wait_queue = WaitQueue::default();
            let waiter = Waiter::new();
            let wk1 = wait_queue.wait_async(&waiter);
            let _wk2 = wait_queue.wait_async(&waiter);
            wk1.cancel();
            wait_queue.notify_all();
            assert!(waiter.wait_until(&current_task, zx::MonotonicInstant::ZERO).is_ok());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn multiple_waiters_cancel_one_other_still_notified() {
        spawn_kernel_and_run(async |current_task| {
            let wait_queue = WaitQueue::default();
            let waiter1 = Waiter::new();
            let waiter2 = Waiter::new();
            let wk1 = wait_queue.wait_async(&waiter1);
            let _wk2 = wait_queue.wait_async(&waiter2);
            wk1.cancel();
            wait_queue.notify_all();
            assert!(waiter1.wait_until(&current_task, zx::MonotonicInstant::ZERO).is_err());
            assert!(waiter2.wait_until(&current_task, zx::MonotonicInstant::ZERO).is_ok());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_wait_queue() {
        spawn_kernel_and_run(async |current_task| {
            let queue = WaitQueue::default();

            let waiters = <[Waiter; 3]>::default();
            waiters.iter().for_each(|w| {
                queue.wait_async(w);
            });

            let woken = || {
                waiters
                    .iter()
                    .filter(|w| w.wait_until(&current_task, zx::MonotonicInstant::ZERO).is_ok())
                    .count()
            };

            const INITIAL_NOTIFY_COUNT: usize = 2;
            let total_waiters = waiters.len();
            queue.notify_unordered_count(INITIAL_NOTIFY_COUNT);
            assert_eq!(INITIAL_NOTIFY_COUNT, woken());

            // Only the remaining (unnotified) waiters should be notified.
            queue.notify_all();
            assert_eq!(total_waiters - INITIAL_NOTIFY_COUNT, woken());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn waiter_kind_abort_handle() {
        spawn_kernel_and_run_sync(|current_task| {
            let mut executor = fuchsia_async::TestExecutor::new();
            let (abort_handle, abort_registration) = futures::stream::AbortHandle::new_pair();
            let abort_handle = Arc::new(abort_handle);
            let waiter_ref = WaiterRef::from_abort_handle(&abort_handle);

            let mut fut = futures::stream::Abortable::new(
                futures::future::pending::<()>(),
                abort_registration,
            );

            assert_matches!(executor.run_until_stalled(&mut fut), std::task::Poll::Pending);

            waiter_ref.interrupt();
            let output = current_task.run_in_state(RunState::Waiter(waiter_ref), move || {
                match executor.run_singlethreaded(&mut fut) {
                    Ok(()) => unreachable!("future never terminates normally"),
                    Err(futures::stream::Aborted) => Ok(()),
                }
            });

            assert_eq!(output, Ok(()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn freeze_with_pending_sigusr1() {
        spawn_kernel_and_run(async |current_task| {
            {
                let mut task_state = current_task.task.write();
                let siginfo = SignalInfo::kernel(SIGUSR1);
                task_state.enqueue_signal(siginfo);
                task_state.set_flags(TaskFlags::SIGNALS_AVAILABLE, true);
            }

            let output: Result<(), Errno> = current_task
                .run_in_state(RunState::Event(InterruptibleEvent::new()), move || {
                    unreachable!("callback should not be called")
                });
            assert_eq!(output, error!(EINTR));

            let output = current_task.run_in_state(RunState::Frozen(Waiter::new()), move || Ok(()));
            assert_eq!(output, Ok(()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn freeze_with_pending_sigkill() {
        spawn_kernel_and_run(async |current_task| {
            {
                let mut task_state = current_task.task.write();
                let siginfo = SignalInfo::kernel(SIGKILL);
                task_state.enqueue_signal(siginfo);
                task_state.set_flags(TaskFlags::SIGNALS_AVAILABLE, true);
            }

            let output: Result<(), _> = current_task
                .run_in_state(RunState::Frozen(Waiter::new()), move || {
                    unreachable!("callback should not be called")
                });
            assert_eq!(output, error!(EINTR));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_async_typed_wait_value_with_handler() {
        spawn_kernel_and_run(async |current_task| {
            let queue: Arc<LockDepMutex<VecDeque<ReadyItem>, EventHandlerReadyQueueLock>> =
                Default::default();
            let handler = EventHandler::Enqueue {
                key: KEY,
                queue: queue.clone(),
                sought_events: FdEvents::all(),
            };
            let waiter = Waiter::new();
            let wait_queue = TypedWaitQueue::<u64>::default();

            let test_value = 100u64;
            let _wait_canceler =
                wait_queue.wait_async_value_with_handler(&waiter, test_value, handler);

            assert!(queue.lock().is_empty());

            // Notify wrong value
            wait_queue.notify_value(test_value + 1);
            assert!(queue.lock().is_empty());

            // Notify right value
            wait_queue.notify_value(test_value);

            waiter.wait(&current_task).expect("wait failed");

            // Result delivered via POLLIN logic in central dispatcher
            let ready_items = queue.lock();
            assert_eq!(ready_items.len(), 1);
            assert!(ready_items[0].events.contains(FdEvents::POLLIN));
        })
        .await;
    }
}
