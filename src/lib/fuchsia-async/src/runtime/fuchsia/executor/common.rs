// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::super::timer::Timers;
use super::atomic_future::hooks::HooksMap;
use super::atomic_future::{AtomicFutureHandle, AttemptPollResult};
use super::packets::{
    PacketReceiver, PacketReceiverMap, RawReceiverRegistration, ReceiverRegistration,
};
use super::scope::ScopeHandle;
use super::time::{BootInstant, MonotonicInstant};
use crate::runtime::instrument::TaskInstrument;
use crossbeam::queue::SegQueue;
use fuchsia_sync::Mutex;
use zx::BootDuration;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::Context;
use std::thread::ThreadId;

pub(crate) const TASK_READY_WAKEUP_ID: u64 = u64::MAX - 1;

thread_local!(
    static EXECUTOR: RefCell<Option<ScopeHandle>> = const { RefCell::new(None) }
);

pub enum ExecutorTime {
    RealTime,
    /// Fake readings used in tests.
    FakeTime {
        // The fake monotonic clock reading.
        mono_reading_ns: AtomicI64,
        // An offset to add to mono_reading_ns to get the reading of the boot
        // clock, disregarding the difference in timelines.
        //
        // We disregard the fact that the reading and offset can not be
        // read atomically, this is usually not relevant in tests.
        mono_to_boot_offset_ns: AtomicI64,
    },
}

enum PollReadyTasksResult {
    NoneReady,
    MoreReady,
    MainTaskCompleted,
}

///  24           16           8            0
///  +------------+------------+------------+
///  |  foreign   |  notified  |  sleeping  |
///  +------------+------------+------------+
///
///  sleeping : the number of threads sleeping
///  notified : the number of notifications posted to wake sleeping threads
///  foreign  : the number of foreign threads processing tasks
#[derive(Clone, Copy, Eq, PartialEq)]
struct ThreadsState(u32);

impl ThreadsState {
    const fn sleeping(&self) -> u8 {
        self.0 as u8
    }

    const fn notified(&self) -> u8 {
        (self.0 >> 8) as u8
    }

    const fn with_sleeping(self, sleeping: u8) -> Self {
        Self((self.0 & !0xff) | sleeping as u32)
    }

    const fn with_notified(self, notified: u8) -> Self {
        Self(self.0 & !0xff00 | (notified as u32) << 8)
    }

    const fn with_foreign(self, foreign: u8) -> Self {
        Self(self.0 & !0xff0000 | (foreign as u32) << 16)
    }
}

#[cfg(test)]
static ACTIVE_EXECUTORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) struct Executor {
    pub(super) port: zx::Port,
    monotonic_timers: Arc<Timers<MonotonicInstant>>,
    boot_timers: Arc<Timers<BootInstant>>,
    pub(super) done: AtomicBool,
    is_local: bool,
    pub(crate) receivers: PacketReceiverMap,

    pub(super) ready_tasks: SegQueue<TaskHandle>,
    time: ExecutorTime,
    // The low byte is the number of threads currently sleeping. The high byte is the number of
    // of wake-up notifications pending.
    pub(super) threads_state: AtomicU32,
    pub(super) num_threads: u8,
    pub(super) polled: AtomicU64,
    // Data that belongs to the user that can be accessed via EHandle::local(). See
    // `TestExecutor::poll_until_stalled`.
    pub(super) owner_data: Mutex<Option<Box<dyn Any + Send>>>,
    pub(super) instrument: Option<Arc<dyn TaskInstrument>>,
    pub(super) hooks_map: HooksMap,
    pub(super) first_thread_id: OnceLock<ThreadId>,

    // The time, in nanoseconds, that the executor was last executing a normal priority task.
    last_active: AtomicI64,
}

impl Executor {
    pub fn new_with_port(
        time: ExecutorTime,
        is_local: bool,
        num_threads: u8,
        port: zx::Port,
        instrument: Option<Arc<dyn TaskInstrument>>,
    ) -> Self {
        #[cfg(test)]
        ACTIVE_EXECUTORS.fetch_add(1, Ordering::Relaxed);

        // Is this a fake-time executor?
        let is_fake = matches!(
            time,
            ExecutorTime::FakeTime { mono_reading_ns: _, mono_to_boot_offset_ns: _ }
        );

        Executor {
            port,
            monotonic_timers: Arc::new(Timers::<MonotonicInstant>::new(is_fake)),
            boot_timers: Arc::new(Timers::<BootInstant>::new(is_fake)),
            done: AtomicBool::new(false),
            is_local,
            receivers: PacketReceiverMap::new(),
            ready_tasks: SegQueue::new(),
            time,
            threads_state: AtomicU32::new(0),
            num_threads,
            polled: AtomicU64::new(0),
            owner_data: Mutex::new(None),
            instrument,
            hooks_map: HooksMap::default(),
            first_thread_id: OnceLock::new(),
            last_active: Default::default(),
        }
    }

    pub fn set_local(root_scope: ScopeHandle) {
        root_scope.executor().first_thread_id.get_or_init(|| std::thread::current().id());
        EXECUTOR.with(|e| {
            let mut e = e.borrow_mut();
            assert!(e.is_none(), "Cannot create multiple Fuchsia Executors");
            *e = Some(root_scope);
        });
    }

    fn poll_ready_tasks(&self, main_task: Option<&TaskHandle>) -> PollReadyTasksResult {
        loop {
            for _ in 0..16 {
                let Some(task) = self.ready_tasks.pop() else {
                    return PollReadyTasksResult::NoneReady;
                };
                let is_main = Some(&task) == main_task;
                let complete = self.try_poll(task);
                if complete && is_main {
                    return PollReadyTasksResult::MainTaskCompleted;
                }
                self.polled.fetch_add(1, Ordering::Relaxed);
            }
            // We didn't finish all the ready tasks. If there are sleeping threads, post a
            // notification to wake one up.
            let mut threads_state = ThreadsState(self.threads_state.load(Ordering::Relaxed));
            loop {
                if threads_state.sleeping() == 0 {
                    // All threads are awake now. Prevent starvation.
                    return PollReadyTasksResult::MoreReady;
                }
                if threads_state.notified() >= threads_state.sleeping() {
                    // All sleeping threads have been notified. Keep going and poll more tasks.
                    break;
                }
                match self.try_notify(threads_state) {
                    Ok(()) => break,
                    Err(s) => threads_state = s,
                }
            }
        }
    }

    pub fn is_local(&self) -> bool {
        self.is_local
    }

    pub fn notify_task_ready(&self) {
        // Only post if there's no thread running (or soon to be running). If we happen to be
        // running on a thread for this executor, then threads_state won't be equal to num_threads,
        // which means notifications only get fired if this is from a non-async thread, or a thread
        // that belongs to a different executor. We use SeqCst ordering here to make sure this load
        // happens *after* the change to ready_tasks and to synchronize with worker_lifecycle.
        let mut threads_state = ThreadsState(self.threads_state.load(Ordering::SeqCst));

        // We only want to notify if there are no pending notifications and there are no other
        // threads running.
        while threads_state == ThreadsState(0).with_sleeping(self.num_threads) {
            match self.try_notify(threads_state) {
                Ok(()) => break,
                Err(s) => threads_state = s,
            }
        }
    }

    /// Tries to notify a thread to wake up. Returns threads_state if it fails.
    fn try_notify(&self, old_threads_state: ThreadsState) -> Result<(), ThreadsState> {
        self.threads_state
            .compare_exchange_weak(
                old_threads_state.0,
                old_threads_state.0 + ThreadsState(0).with_notified(1).0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .map(|_| self.notify_id(TASK_READY_WAKEUP_ID))
            .map_err(ThreadsState)
    }

    pub fn wake_one_thread(&self) {
        let mut threads_state = ThreadsState(self.threads_state.load(Ordering::Relaxed));
        let current_sleeping = threads_state.sleeping();
        if current_sleeping == 0 {
            return;
        }
        while threads_state.notified() == 0 && threads_state.sleeping() >= current_sleeping {
            match self.try_notify(threads_state) {
                Ok(()) => break,
                Err(s) => threads_state = s,
            }
        }
    }

    pub fn notify_id(&self, id: u64) {
        let up = zx::UserPacket::from_u8_array([0; 32]);
        let packet = zx::Packet::from_user_packet(id, 0 /* status??? */, up);
        if let Err(e) = self.port.queue(&packet) {
            // TODO: logging
            eprintln!("Failed to queue notify in port: {e:?}");
        }
    }

    /// Returns the current reading of the monotonic clock.
    ///
    /// For test executors running in fake time, returns the reading of the
    /// fake monotonic clock.
    pub fn now(&self) -> MonotonicInstant {
        match &self.time {
            ExecutorTime::RealTime => MonotonicInstant::from_zx(zx::MonotonicInstant::get()),
            ExecutorTime::FakeTime { mono_reading_ns: t, .. } => {
                MonotonicInstant::from_nanos(t.load(Ordering::Relaxed))
            }
        }
    }

    /// Returns the current reading of the boot clock.
    ///
    /// For test executors running in fake time, returns the reading of the
    /// fake boot clock.
    pub fn boot_now(&self) -> BootInstant {
        match &self.time {
            ExecutorTime::RealTime => BootInstant::from_zx(zx::BootInstant::get()),

            ExecutorTime::FakeTime { mono_reading_ns: t, mono_to_boot_offset_ns } => {
                // The two atomic values are loaded one after the other. This should
                // not normally be an issue in tests.
                let fake_mono_now = MonotonicInstant::from_nanos(t.load(Ordering::Relaxed));
                let boot_offset_ns = mono_to_boot_offset_ns.load(Ordering::Relaxed);
                BootInstant::from_nanos(fake_mono_now.into_nanos() + boot_offset_ns)
            }
        }
    }

    /// Sets the reading of the fake monotonic clock.
    ///
    /// # Panics
    ///
    /// If called on an executor that runs in real time.
    pub fn set_fake_time(&self, new: MonotonicInstant) {
        let boot_offset_ns = match &self.time {
            ExecutorTime::RealTime => {
                panic!("Error: called `set_fake_time` on an executor using actual time.")
            }
            ExecutorTime::FakeTime { mono_reading_ns: t, mono_to_boot_offset_ns } => {
                t.store(new.into_nanos(), Ordering::Relaxed);
                mono_to_boot_offset_ns.load(Ordering::Relaxed)
            }
        };
        self.monotonic_timers.maybe_notify(new);

        // Changing fake time also affects boot time.  Notify boot clocks as well.
        let new_boot_time = BootInstant::from_nanos(new.into_nanos() + boot_offset_ns);
        self.boot_timers.maybe_notify(new_boot_time);
    }

    // Sets a new offset between boot and monotonic time.
    //
    // Only works for executors operating in fake time.
    // The change in the fake offset will wake expired boot timers.
    pub fn set_fake_boot_to_mono_offset(&self, offset: BootDuration) {
        let mono_now_ns = match &self.time {
            ExecutorTime::RealTime => {
                panic!(
                    "Error: called `set_fake_boot_to_mono_offset` on an executor using actual time."
                )
            }
            ExecutorTime::FakeTime { mono_reading_ns: t, mono_to_boot_offset_ns: b } => {
                // We ignore the non-atomic update between b and t, it is likely
                // not relevant in tests.
                b.store(offset.into_nanos(), Ordering::Relaxed);
                t.load(Ordering::Relaxed)
            }
        };
        let new_boot_now = BootInstant::from_nanos(mono_now_ns) + offset;
        self.boot_timers.maybe_notify(new_boot_now);
    }

    /// Returns `true` if this executor is running in real time.  Returns
    /// `false` if this executor si running in fake time.
    pub fn is_real_time(&self) -> bool {
        matches!(self.time, ExecutorTime::RealTime)
    }

    /// Must be called before `on_parent_drop`.
    ///
    /// Done flag must be set before dropping packet receivers
    /// so that future receivers that attempt to deregister themselves
    /// know that it's okay if their entries are already missing.
    pub fn mark_done(&self) {
        self.done.store(true, Ordering::SeqCst);

        // Make sure there's at least one notification outstanding per thread to wake up all
        // workers. This might be more notifications than required, but this way we don't have to
        // worry about races where tasks are just about to sleep; when a task receives the
        // notification, it will check done and terminate.
        let mut threads_state = ThreadsState(self.threads_state.load(Ordering::Relaxed));
        let num_threads = self.num_threads;
        loop {
            let notified = threads_state.notified();
            if notified >= num_threads {
                break;
            }
            match self.threads_state.compare_exchange_weak(
                threads_state.0,
                threads_state.with_notified(num_threads).0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    for _ in notified..num_threads {
                        self.notify_id(TASK_READY_WAKEUP_ID);
                    }
                    return;
                }
                Err(old) => threads_state = ThreadsState(old),
            }
        }
    }

    /// Notes about the lifecycle of an Executor.
    ///
    /// a) The Executor stands as the only way to run a reactor based on a Fuchsia port, but the
    /// lifecycle of the port itself is not currently tied to it. Executor vends clones of its
    /// inner Arc structure to all receivers, so we don't have a type-safe way of ensuring that
    /// the port is dropped alongside the Executor as it should.
    /// TODO(https://fxbug.dev/42154828): Ensure the port goes away with the executor.
    ///
    /// b) The Executor's lifetime is also tied to the thread-local variable pointing to the
    /// "current" executor being set, and that's unset when the executor is dropped.
    ///
    /// Point (a) is related to "what happens if I use a receiver after the executor is dropped",
    /// and point (b) is related to "what happens when I try to create a new receiver when there
    /// is no executor".
    ///
    /// Tokio, for example, encodes the lifetime of the reactor separately from the thread-local
    /// storage [1]. And the reactor discourages usage of strong references to it by vending weak
    /// references to it [2] instead of strong.
    ///
    /// There are pros and cons to both strategies. For (a), tokio encourages (but doesn't
    /// enforce [3]) type-safety by vending weak pointers, but those add runtime overhead when
    /// upgrading pointers. For (b) the difference mostly stand for "when is it safe to use IO
    /// objects/receivers". Tokio says it's only safe to use them whenever a guard is in scope.
    /// Fuchsia-async says it's safe to use them when a fuchsia_async::Executor is still in scope
    /// in that thread.
    ///
    /// This acts as a prelude to the panic encoded in Executor::drop when receivers haven't
    /// unregistered themselves when the executor drops. The choice to panic was made based on
    /// patterns in fuchsia-async that may come to change:
    ///
    /// - Executor vends strong references to itself and those references are *stored* by most
    ///   receiver implementations (as opposed to reached out on TLS every time).
    /// - Fuchsia-async objects return zx::Status on wait calls, there isn't an appropriate and
    ///   easy to understand error to return when polling on an extinct executor.
    /// - All receivers are implemented in this crate and well-known.
    ///
    /// [1]: https://docs.rs/tokio/1.5.0/tokio/runtime/struct.Runtime.html#method.enter
    /// [2]: https://github.com/tokio-rs/tokio/blob/b42f21ec3e212ace25331d0c13889a45769e6006/tokio/src/signal/unix/driver.rs#L35
    /// [3]: by returning an upgraded Arc, tokio trusts callers to not "use it for too long", an
    /// opaque non-clone-copy-or-send guard would be stronger than this. See:
    /// https://github.com/tokio-rs/tokio/blob/b42f21ec3e212ace25331d0c13889a45769e6006/tokio/src/io/driver/mod.rs#L297
    pub fn on_parent_drop(&self, root_scope: &ScopeHandle) {
        // Drop all tasks.
        // Any use of fasync::unblock can involve a waker. Wakers hold weak references to tasks, but
        // as part of waking, there's an upgrade to a strong reference, so for a small amount of
        // time `fasync::unblock` can hold a strong reference to a task which in turn holds the
        // future for the task which in turn could hold references to receivers, which, if we did
        // nothing about it, would trip the assertion below. For that reason, we forcibly drop the
        // task futures here.
        root_scope.drop_all_tasks();

        // Drop all of the uncompleted tasks
        while self.ready_tasks.pop().is_some() {}

        // Deregister the timer receivers so that we can perform the check below.
        self.monotonic_timers.deregister();
        self.boot_timers.deregister();

        // Do not allow any receivers to outlive the executor. That's very likely a bug waiting to
        // happen. See discussion above.
        //
        // If you're here because you hit this panic check your code for:
        //
        // - A struct that contains a fuchsia_async::Executor NOT in the last position (last
        // position gets dropped last: https://doc.rust-lang.org/reference/destructors.html).
        //
        // - A function scope that contains a fuchsia_async::Executor NOT in the first position
        // (first position in function scope gets dropped last:
        // https://doc.rust-lang.org/reference/destructors.html?highlight=scope#drop-scopes).
        //
        // - A function that holds a `fuchsia_async::Executor` in scope and whose last statement
        // contains a temporary (temporaries are dropped after the function scope:
        // https://doc.rust-lang.org/reference/destructors.html#temporary-scopes). This usually
        // looks like a `match` statement at the end of the function without a semicolon.
        //
        // - Storing channel and FIDL objects in static variables.
        //
        // - fuchsia_async::unblock calls that move channels or FIDL objects to another thread.
        assert!(self.receivers.is_empty(), "receivers must not outlive their executor");

        // Remove the thread-local executor set in `new`.
        EHandle::rm_local();
    }

    // The debugger looks for this function on the stack, so if its (fully-qualified) name changes,
    // the debugger needs to be updated.
    // LINT.IfChange
    pub fn worker_lifecycle<const UNTIL_STALLED: bool>(
        self: &Arc<Executor>,
        // The main task should be specified for a single-threaded executor, but it isn't necessary
        // for a multi-threaded executor since it has an independent mechanism for tracking when
        // the main task terminates.
        // TODO(https://fxbug.dev/481030722) remove this parameter
        main_task: Option<&TaskHandle>,
    ) {
        // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)

        assert!(
            !self.is_local() || self.first_thread_id.get() == Some(&std::thread::current().id())
        );

        self.monotonic_timers.register(self);
        self.boot_timers.register(self);

        loop {
            // Keep track of whether we are considered asleep.
            let mut sleeping = false;

            match self.poll_ready_tasks(main_task) {
                PollReadyTasksResult::NoneReady => {
                    // No more tasks, indicate we are sleeping. We use SeqCst ordering because we
                    // want this change here to happen *before* we check ready_tasks below. This
                    // synchronizes with notify_task_ready which is called *after* a task is added
                    // to ready_tasks.
                    const ONE_SLEEPING: ThreadsState = ThreadsState(0).with_sleeping(1);
                    self.threads_state.fetch_add(ONE_SLEEPING.0, Ordering::SeqCst);
                    // Check ready tasks again. If a task got posted, wake up. This has to be done
                    // because a notification won't get sent if there is at least one active thread
                    // so there's a window between the preceding two lines where a task could be
                    // made ready and a notification is not sent because it looks like there is at
                    // least one thread running.
                    if self.ready_tasks.is_empty() {
                        sleeping = true;
                    } else {
                        // We lost a race, we're no longer sleeping.
                        self.threads_state.fetch_sub(ONE_SLEEPING.0, Ordering::Relaxed);
                    }
                }
                PollReadyTasksResult::MoreReady => {}
                PollReadyTasksResult::MainTaskCompleted => return,
            }

            // Check done here after updating threads_state to avoid shutdown races.
            if self.done.load(Ordering::SeqCst) {
                return;
            }

            enum Work {
                None,
                Packet(zx::Packet),
                Stalled,
            }

            let mut notified = false;
            let work = {
                // If we're considered awake choose INFINITE_PAST which will make the wait call
                // return immediately.  Otherwise, wait until a packet arrives.
                let deadline = if !sleeping || UNTIL_STALLED {
                    zx::Instant::INFINITE_PAST
                } else {
                    zx::Instant::INFINITE
                };

                match self.port.wait(deadline) {
                    Ok(packet) => {
                        if packet.key() == TASK_READY_WAKEUP_ID {
                            notified = true;
                            Work::None
                        } else {
                            Work::Packet(packet)
                        }
                    }
                    Err(zx::Status::TIMED_OUT) => {
                        if !UNTIL_STALLED || !sleeping {
                            Work::None
                        } else {
                            Work::Stalled
                        }
                    }
                    Err(status) => {
                        panic!("Error calling port wait: {status:?}");
                    }
                }
            };

            let threads_state_sub =
                ThreadsState(0).with_sleeping(sleeping as u8).with_notified(notified as u8);
            if threads_state_sub.0 > 0 {
                self.threads_state.fetch_sub(threads_state_sub.0, Ordering::Relaxed);
            }

            match work {
                Work::Packet(packet) => self.receivers.receive_packet(packet.key(), packet),
                Work::None => {}
                Work::Stalled => return,
            }
        }
    }

    /// Drops the main task.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the executor isn't running.
    pub(super) unsafe fn drop_main_task(&self, root_scope: &ScopeHandle, task: &TaskHandle) {
        unsafe { root_scope.drop_task_unchecked(task) };
    }

    fn try_poll(&self, task: TaskHandle) -> bool {
        let task_waker = task.waker();
        let poll_result = TaskHandle::set_current_with(&task, || {
            task.try_poll(&mut Context::from_waker(&task_waker))
        });
        if !task.is_low_priority() {
            self.last_active.store(self.now().into_nanos(), Ordering::Relaxed);
        }
        match poll_result {
            AttemptPollResult::Yield => {
                self.ready_tasks.push(task);
                false
            }
            AttemptPollResult::IFinished | AttemptPollResult::Aborted => {
                task.scope().task_did_finish(&task);
                true
            }
            _ => false,
        }
    }

    /// Returns the monotonic timers.
    pub fn monotonic_timers(&self) -> &Timers<MonotonicInstant> {
        &self.monotonic_timers
    }

    /// Returns the boot timers.
    pub fn boot_timers(&self) -> &Timers<BootInstant> {
        &self.boot_timers
    }

    fn poll_tasks(&self, callback: impl FnOnce(), main_task: Option<&TaskHandle>) {
        assert!(!self.is_local);

        // Increment the count of foreign threads.
        const ONE_FOREIGN: ThreadsState = ThreadsState(0).with_foreign(1);
        self.threads_state.fetch_add(ONE_FOREIGN.0, Ordering::Relaxed);

        callback();

        // Poll up to 16 tasks.
        for _ in 0..16 {
            let Some(task) = self.ready_tasks.pop() else {
                break;
            };
            let is_main = Some(&task) == main_task;
            if self.try_poll(task) && is_main {
                break;
            }
            self.polled.fetch_add(1, Ordering::Relaxed);
        }

        let mut threads_state = ThreadsState(
            self.threads_state.fetch_sub(ONE_FOREIGN.0, Ordering::SeqCst) - ONE_FOREIGN.0,
        );

        if !self.ready_tasks.is_empty() {
            // There are tasks still ready to run, so wake up a thread if all the other threads are
            // sleeping.
            while threads_state == ThreadsState(0).with_sleeping(self.num_threads) {
                match self.try_notify(threads_state) {
                    Ok(()) => break,
                    Err(s) => threads_state = s,
                }
            }
        }
    }

    pub fn task_is_ready(&self, task: TaskHandle) {
        self.ready_tasks.push(task);
        self.notify_task_ready();
    }

    pub(crate) fn last_active(&self) -> MonotonicInstant {
        MonotonicInstant::from_nanos(self.last_active.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
impl Drop for Executor {
    fn drop(&mut self) {
        ACTIVE_EXECUTORS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A handle to an executor.
#[derive(Clone)]
pub struct EHandle {
    // LINT.IfChange
    pub(super) root_scope: ScopeHandle,
    // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)
}

impl fmt::Debug for EHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EHandle").field("port", &self.inner().port).finish()
    }
}

impl EHandle {
    /// Returns the thread-local executor.
    ///
    /// # Panics
    ///
    /// If called outside the context of an active async executor.
    pub fn local() -> Self {
        let root_scope = EXECUTOR
            .with(|e| e.borrow().as_ref().map(|x| x.clone()))
            .expect("Fuchsia Executor must be created first");

        EHandle { root_scope }
    }

    /// Set the fake time to a given value.
    ///
    /// # Panics
    ///
    /// If the executor was not created with fake time.
    pub fn set_fake_time(&self, t: MonotonicInstant) {
        self.inner().set_fake_time(t)
    }

    /// Gets the current time as the executor sees it. May be fake time if the executor is running
    /// on fake time.
    pub fn now(&self) -> MonotonicInstant {
        self.inner().now()
    }

    pub(super) fn rm_local() {
        EXECUTOR.with(|e| *e.borrow_mut() = None);
    }

    /// The root scope of the executor.
    ///
    /// This can be used to spawn tasks that live as long as the executor, and
    /// to create shorter-lived child scopes.
    ///
    /// Most users should create an owned scope with
    /// [`Scope::new_with_name`][crate::Scope::new_with_name] instead of using this method.
    pub fn global_scope(&self) -> &ScopeHandle {
        &self.root_scope
    }

    /// Get a reference to the Fuchsia `zx::Port` being used to listen for events.
    pub fn port(&self) -> &zx::Port {
        &self.inner().port
    }

    /// Registers a `PacketReceiver` with the executor and returns a registration.
    /// The `PacketReceiver` will be deregistered when the `Registration` is dropped.
    pub fn register_receiver<T: PacketReceiver>(&self, receiver: T) -> ReceiverRegistration<T> {
        self.inner().receivers.register(self.inner().clone(), receiver)
    }

    /// Registers a pinned `RawPacketReceiver` with the executor.
    ///
    /// The registration will be deregistered when dropped.
    ///
    /// NOTE: Unlike with `register_receiver`, `receive_packet` will be called whilst a lock is
    /// held, so it is not safe to register or unregister receivers at that time.
    pub fn register_pinned<T: PacketReceiver>(
        &self,
        raw_registration: Pin<&mut RawReceiverRegistration<T>>,
    ) {
        self.inner().receivers.register_pinned(self.clone(), raw_registration);
    }

    #[inline(always)]
    pub(crate) fn inner(&self) -> &Arc<Executor> {
        self.root_scope.executor()
    }

    /// Spawn a new task to be run on this executor.
    ///
    /// Tasks spawned using this method must be thread-safe (implement the `Send` trait), as they
    /// may be run on either a singlethreaded or multithreaded executor.
    pub fn spawn_detached(&self, future: impl Future<Output = ()> + Send + 'static) {
        self.global_scope().spawn(future);
    }

    /// Spawn a new task to be run on this executor.
    ///
    /// This is similar to the `spawn_detached` method, but tasks spawned using this method do not
    /// have to be threads-safe (implement the `Send` trait). In return, this method requires that
    /// this executor is a LocalExecutor.
    pub fn spawn_local_detached(&self, future: impl Future<Output = ()> + 'static) {
        self.global_scope().spawn_local(future);
    }

    pub(crate) fn mono_timers(&self) -> &Arc<Timers<MonotonicInstant>> {
        &self.inner().monotonic_timers
    }

    pub(crate) fn boot_timers(&self) -> &Arc<Timers<BootInstant>> {
        &self.inner().boot_timers
    }

    /// Calls `callback` in the context of the executor and then polls (a limited number of) tasks
    /// that are ready to run.  If tasks remain ready and no other threads are running, a thread
    /// will be woken.  This can end up being a performance win in the case that the queue can be
    /// cleared without needing to wake any other thread.
    ///
    /// # Panics
    ///
    /// If called on a single-threaded executor or if this thread is a thread managed by the
    /// executor.
    pub fn poll_tasks(&self, callback: impl FnOnce()) {
        EXECUTOR.with(|e| {
            assert!(
                e.borrow_mut().replace(self.root_scope.clone()).is_none(),
                "This thread is already associated with an executor"
            );
        });

        // `poll_tasks` should only ever be used on a multi-threaded executor, it's safe to pass
        // None.
        self.inner().poll_tasks(callback, None);

        EXECUTOR.with(|e| *e.borrow_mut() = None);
    }
}

// AtomicFutureHandle can have a lifetime (for local executors we allow the main task to have a
// non-static lifetime).  The executor doesn't handle this though; the executor just assumes all
// tasks have the 'static lifetime.  It's up to the local executor to extend the lifetime and make
// it safe.
pub type TaskHandle = AtomicFutureHandle<'static>;

thread_local! {
    static CURRENT_TASK: Cell<*const TaskHandle> = const { Cell::new(std::ptr::null()) };
}

impl TaskHandle {
    pub(crate) fn with_current<R>(f: impl FnOnce(Option<&TaskHandle>) -> R) -> R {
        CURRENT_TASK.with(|cur| {
            let cur = cur.get();
            let cur = unsafe { cur.as_ref() };
            f(cur)
        })
    }

    fn set_current_with<R>(task: &TaskHandle, f: impl FnOnce() -> R) -> R {
        CURRENT_TASK.with(|cur| {
            cur.set(task);
            let result = f();
            cur.set(std::ptr::null());
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTIVE_EXECUTORS, EHandle};
    use crate::{LocalExecutorBuilder, SendExecutorBuilder};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn test_no_leaks() {
        std::thread::spawn(|| SendExecutorBuilder::new().num_threads(1).build().run(async {}))
            .join()
            .unwrap();

        assert_eq!(ACTIVE_EXECUTORS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn poll_tasks() {
        SendExecutorBuilder::new().num_threads(1).build().run(async {
            let ehandle = EHandle::local();

            // This will tie up the executor's only running thread which ensures that the task
            // we spawn below can only run on the foreign thread.
            std::thread::spawn(move || {
                let ran = Arc::new(AtomicU64::new(0));
                ehandle.poll_tasks(|| {
                    let ran = ran.clone();
                    ehandle.spawn_detached(async move {
                        ran.fetch_add(1, Ordering::Relaxed);
                    });
                });

                // The spawned task should have run in this thread.
                assert_eq!(ran.load(Ordering::Relaxed), 1);
            })
            .join()
            .unwrap();
        });
    }

    #[test]
    #[should_panic]
    fn spawn_local_from_different_thread() {
        let _executor = LocalExecutorBuilder::new().build();
        let ehandle = EHandle::local();
        let _ = std::thread::spawn(move || {
            ehandle.spawn_local_detached(async {});
        })
        .join();
    }
}
