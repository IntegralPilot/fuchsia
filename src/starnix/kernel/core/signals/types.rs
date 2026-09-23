// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::{MemoryAccessor, MemoryAccessorExt};
use crate::task::{CurrentTask, Pid, Task, ThreadGroupReadGuard, WaitQueue, Waiter};
use crate::time::IntervalTimerHandle;
use fuchsia_trace::{TraceCategoryContext, trace_site_t};

use starnix_sync::{LockDepRwLock, SignalActionsLock};
use starnix_types::arch::ArchWidth;
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::{SigSet, Signal, UNBLOCKABLE_SIGNALS, UncheckedSignal};
use starnix_uapi::union::struct_with_union_into_bytes;
use starnix_uapi::user_address::{ArchSpecific, MultiArchUserRef, UserAddress};
use starnix_uapi::{
    SI_KERNEL, SI_MAX_SIZE, SIG_IGN, SIGEV_NONE, SIGEV_SIGNAL, SIGEV_THREAD, SIGEV_THREAD_ID,
    c_int, c_uint, errno, error, sigaction_t, sigaltstack, sigevent, sigval_t, tid_t, uaddr, uapi,
    uid_t,
};
use static_assertions::const_assert;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::{Arc, Weak};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const TRACE_CATEGORY: &'static str = "starnix:signals";

pub const SI_HEADER_SIZE: usize = std::mem::size_of::<SignalInfoHeader>();

// Rust will let us do this cast in a const assignment but not in a const generic constraint.
pub const SI_MAX_SIZE_AS_USIZE: usize = SI_MAX_SIZE as usize;
pub const SI_DETAIL_SIZE: usize = SI_MAX_SIZE_AS_USIZE - SI_HEADER_SIZE;

/// Internal signal that cannot be masked, blocked or ignored.
#[derive(Debug)]
pub enum KernelSignal {
    Freeze(Waiter),
}

/// Signal info wrapper around user and internal signals.
pub enum KernelSignalInfo {
    User(SignalInfo),
    Freeze(Waiter),
}

impl From<KernelSignal> for KernelSignalInfo {
    fn from(value: KernelSignal) -> Self {
        match value {
            KernelSignal::Freeze(waiter) => KernelSignalInfo::Freeze(waiter),
        }
    }
}

impl From<SignalInfo> for KernelSignalInfo {
    fn from(value: SignalInfo) -> Self {
        KernelSignalInfo::User(value)
    }
}

/// `SignalActions` contains a `sigaction_t` for each valid signal.
#[derive(Debug)]
pub struct SignalActions {
    actions: LockDepRwLock<[sigaction_t; Signal::NUM_SIGNALS as usize + 1], SignalActionsLock>,
}

impl SignalActions {
    /// Returns a collection of `sigaction_t`s that contains default values for each signal.
    pub fn default() -> Arc<SignalActions> {
        Arc::new(SignalActions {
            actions: [sigaction_t::default(); Signal::NUM_SIGNALS as usize + 1].into(),
        })
    }

    /// Returns the `sigaction_t` that is currently set for `signal`.
    pub fn get(&self, signal: Signal) -> sigaction_t {
        // This is safe, since the actions always contain a value for each signal.
        self.actions.read()[signal.number() as usize]
    }

    /// Update the action for `signal`. Returns the previously configured action.
    pub fn set(&self, signal: Signal, new_action: sigaction_t) -> sigaction_t {
        let mut actions = self.actions.write();
        let old_action = actions[signal.number() as usize];
        actions[signal.number() as usize] = new_action;
        old_action
    }

    pub fn fork(&self) -> Arc<SignalActions> {
        Arc::new(SignalActions { actions: (*self.actions.read()).into() })
    }

    /// Resets all signal actions to the default, except that ignored
    /// signals remain ignored.
    pub fn reset_for_exec(&self) {
        for action in self.actions.write().iter_mut() {
            let handler = action.sa_handler;
            *action = sigaction_t::default();
            if handler == SIG_IGN {
                action.sa_handler = SIG_IGN;
            }
        }
    }

    /// Resets all signal actions to the default.
    pub fn reset_to_default(&self) {
        for action in self.actions.write().iter_mut() {
            *action = sigaction_t::default();
        }
    }
}

#[derive(Default)]
pub struct QueuedSignals {
    /// The queue of standard signals for the task.
    queue: VecDeque<SignalInfo>,

    /// Real-time signals queued for the task. Unlike standard signals there may be more than one
    /// instance of the same real-time signal in the queue. POSIX requires real-time signals
    /// with lower values to be delivered first. `enqueue()` ensures proper ordering when adding
    /// new elements. There are no ordering requirements for standard signals. We always dequeue
    /// standard signals first. This matches Linux behavior.
    rt_queue: VecDeque<SignalInfo>,
}

impl QueuedSignals {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn enqueue(&mut self, siginfo: SignalInfo) {
        if siginfo.signal.is_real_time() {
            // Real-time signals are stored in `rt_queue` in the order they will be delivered,
            // i.e. they sorted by the signal number. Signals with the same number must be
            // delivered in the order they were queued. Use binary search to find the right
            // position to insert the signal. Note that the comparator return `Less` when the
            // signal is the same.
            let pos = self
                .rt_queue
                .binary_search_by(|v| {
                    if v.signal.number() <= siginfo.signal.number() {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                })
                .expect_err("Invalid result from binary_search_by()");
            self.rt_queue.insert(pos, siginfo);
        } else {
            // Don't queue duplicate standard signals.
            if self.queue.iter().any(move |info| info.signal == siginfo.signal) {
                return;
            }
            self.queue.push_back(siginfo);
        };
    }

    /// Used by ptrace to provide a replacement for the signal that might have been
    /// delivered when the task entered signal-delivery-stop.
    pub fn jump_queue(&mut self, siginfo: SignalInfo) {
        self.queue.push_front(siginfo);
    }

    /// Finds the next queued signal where the given function returns true, removes it from the
    /// queue, and returns it.
    pub fn take_next_where<F>(&mut self, predicate: F) -> Option<SignalInfo>
    where
        F: Fn(&SignalInfo) -> bool,
    {
        // Find the first signal passing `predicate`, prioritizing standard signals.
        if let Some(index) = self.queue.iter().position(&predicate) {
            return self.queue.remove(index);
        }
        if let Some(index) = self.rt_queue.iter().position(predicate) {
            return self.rt_queue.remove(index);
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.rt_queue.is_empty()
    }

    /// Returns whether any signals are queued and not blocked by the given mask.
    pub fn is_any_allowed_by_mask(&self, mask: SigSet) -> bool {
        self.iter().any(|sig| !mask.has_signal(sig.signal))
    }

    /// Returns an iterator over all the pending signals.
    fn iter(&self) -> impl Iterator<Item = &SignalInfo> {
        self.queue.iter().chain(self.rt_queue.iter())
    }

    /// Iterates over queued signals with the given number.
    fn iter_queued_by_number(&self, signal: Signal) -> impl Iterator<Item = &SignalInfo> {
        self.iter().filter(move |info| info.signal == signal)
    }

    /// Returns the set of currently pending signals, both standard and real time.
    pub fn pending(&self) -> SigSet {
        self.iter().fold(SigSet::default(), |pending, signal| pending | signal.signal.into())
    }

    /// Tests whether a signal with the given number is in the queue.
    pub fn has_queued(&self, signal: Signal) -> bool {
        self.iter_queued_by_number(signal).next().is_some()
    }

    pub fn num_queued(&self) -> usize {
        self.queue.len() + self.rt_queue.len()
    }

    #[cfg(test)]
    pub fn queued_count(&self, signal: Signal) -> usize {
        self.iter_queued_by_number(signal).count()
    }
}

/// Per-task signal handling state.
#[derive(Default)]
pub struct SignalState {
    // See https://man7.org/linux/man-pages/man2/sigaltstack.2.html
    pub alt_stack: Option<sigaltstack>,

    /// Wait queue for signalfd and sigtimedwait. Signaled whenever a signal is added to the queue.
    pub signal_wait: WaitQueue,

    /// The signal mask of the task.
    ///
    /// It is the set of signals whose delivery is currently blocked for the caller.
    /// See https://man7.org/linux/man-pages/man7/signal.7.html
    mask: SigSet,

    /// The signal mask that should be restored by the signal handling machinery, after dequeuing
    /// a signal.
    ///
    /// Some syscalls apply a temporary signal mask by setting `SignalState.mask` during the wait.
    /// This means that the mask must be set to the temporary mask when the signal is dequeued,
    /// which is done by the syscall dispatch loop before returning to userspace. After the signal
    /// is dequeued `mask` can be reset to `saved_mask`.
    saved_mask: Option<SigSet>,

    /// The queue of signals for the task.
    queue: QueuedSignals,
}

impl SignalState {
    pub fn with_mask(mask: SigSet) -> Self {
        Self { mask, ..Default::default() }
    }

    /// Sets the signal mask of the state, and returns the old signal mask.
    pub fn set_mask(&mut self, signal_mask: SigSet) -> SigSet {
        let old_mask = self.mask;
        self.mask = signal_mask & !UNBLOCKABLE_SIGNALS;
        old_mask
    }

    /// Sets the signal mask of the state temporarily, until the signal machinery has completed its
    /// next dequeue operation. This can be used by syscalls that want to change the signal mask
    /// during a wait, but want the signal mask to be reset before returning back to userspace after
    /// the wait.
    pub fn set_temporary_mask(&mut self, signal_mask: SigSet) {
        assert!(self.saved_mask.is_none());
        self.saved_mask = Some(self.mask);
        self.mask = signal_mask & !UNBLOCKABLE_SIGNALS;
    }

    /// Restores the signal mask to what it was before the previous call to `set_temporary_mask`.
    /// If there is no saved mask, the mask is left alone.
    pub fn restore_mask(&mut self) {
        if let Some(mask) = self.saved_mask {
            self.mask = mask;
            self.saved_mask = None;
        }
    }

    pub fn mask(&self) -> SigSet {
        self.mask
    }

    pub fn saved_mask(&self) -> Option<SigSet> {
        self.saved_mask
    }

    pub fn enqueue(&mut self, siginfo: SignalInfo) {
        self.queue.enqueue(siginfo);
        self.signal_wait.notify_all();
    }

    /// Used by ptrace to provide a replacement for the signal that might have been
    /// delivered when the task entered signal-delivery-stop.
    pub fn jump_queue(&mut self, siginfo: SignalInfo) {
        self.queue.jump_queue(siginfo);
        self.signal_wait.notify_all();
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Finds the next queued signal where the given function returns true, removes it from the
    /// queue, and returns it.
    pub fn take_next_where<F>(&mut self, predicate: F) -> Option<SignalInfo>
    where
        F: Fn(&SignalInfo) -> bool,
    {
        self.queue.take_next_where(predicate)
    }

    /// Returns whether any signals are pending (queued and not blocked).
    pub fn is_any_pending(&self) -> bool {
        self.queue.is_any_allowed_by_mask(self.mask)
    }

    /// Returns whether any signals are queued and not blocked by the given mask.
    pub fn is_any_allowed_by_mask(&self, mask: SigSet) -> bool {
        self.queue.is_any_allowed_by_mask(mask)
    }

    pub fn pending(&self) -> SigSet {
        self.queue.pending()
    }

    /// Tests whether a signal with the given number is in the queue.
    pub fn has_queued(&self, signal: Signal) -> bool {
        self.queue.has_queued(signal)
    }

    pub fn num_queued(&self) -> usize {
        self.queue.num_queued()
    }

    pub fn reset_to_default(&mut self) {
        *self = SignalState::default();
    }

    #[cfg(test)]
    pub fn queued_count(&self, signal: Signal) -> usize {
        self.queue.queued_count(signal)
    }
}

// Ensure siginfo has the same size on all architecture and that the header is the same.
uapi::check_arch_independent_layout! {
    siginfo {
        __bindgen_anon_1,
        __bindgen_anon_1.__bindgen_anon_1,
        __bindgen_anon_1._si_pad,
        __bindgen_anon_1.__bindgen_anon_1.si_signo,
        __bindgen_anon_1.__bindgen_anon_1.si_errno,
        __bindgen_anon_1.__bindgen_anon_1.si_code,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C)]
pub struct SignalInfoHeader {
    pub signo: u32,
    pub errno: i32,
    pub code: i32,
}

/// Represents a `siginfo_t` structure that has been read from userspace but not yet validated.
pub struct UncheckedSignalInfo {
    header: SignalInfoHeader,
    data: [u8; SI_DETAIL_SIZE],

    /// The task that sent this signal. None for kernel-originated signals.
    sender: Option<Weak<Task>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntoSignalInfoOptions {
    /// Do not check the signo field of the siginfo_t structure.
    None,

    /// Check that the signo field of the siginfo_t structure matches the given signal.
    /// Returns EINVAL if they do not match.
    CheckSigno,
}

impl UncheckedSignalInfo {
    /// Reads a `siginfo_t` structure from the given userspace address.
    ///
    /// # Args
    /// * `current_task`: The task from which to read the memory.
    /// * `siginfo_ref`: The userspace address of the `siginfo_t` structure.
    ///
    /// # Returns
    /// The `UncheckedSignalInfo` structure read from userspace.
    pub fn read_from_siginfo(
        current_task: &CurrentTask,
        siginfo_ref: UserAddress,
    ) -> Result<Self, Errno> {
        let siginfo_mem = current_task.read_memory_to_array::<SI_MAX_SIZE_AS_USIZE>(siginfo_ref)?;
        let header = SignalInfoHeader::read_from_bytes(&siginfo_mem[..SI_HEADER_SIZE]).unwrap();
        let mut data = [0u8; SI_DETAIL_SIZE];
        data.copy_from_slice(&siginfo_mem[SI_HEADER_SIZE..]);
        Ok(Self { header, data, sender: Some(current_task.weak_task()) })
    }

    pub fn signo(&self) -> u32 {
        self.header.signo
    }

    pub fn code(&self) -> i32 {
        self.header.code
    }

    /// Converts this unchecked signal info into a `SignalInfo` with the given signal.
    ///
    /// Some syscalls pass in a signal number as well as a siginfo_t structure. Depending on the
    /// syscall, the signo field of the siginfo_t structure is either ignored, or checked for
    /// consistency with the signal number passed in.
    #[track_caller]
    pub fn into_signal_info(
        self,
        signal: Signal,
        options: IntoSignalInfoOptions,
    ) -> Result<SignalInfo, Errno> {
        if matches!(options, IntoSignalInfoOptions::CheckSigno) && self.signo() != signal.number() {
            return error!(EINVAL);
        }
        Ok(SignalInfo::new(
            signal,
            self.header.errno,
            self.header.code,
            SignalDetail::Raw { data: self.data },
            false,
            self.sender,
        ))
    }
}

impl TryFrom<UncheckedSignalInfo> for SignalInfo {
    type Error = Errno;

    #[track_caller]
    fn try_from(siginfo: UncheckedSignalInfo) -> Result<Self, Self::Error> {
        let signal = Signal::try_from(UncheckedSignal::from(siginfo.signo()))?;
        siginfo.into_signal_info(signal, IntoSignalInfoOptions::None)
    }
}

#[derive(Debug)]
pub struct SignalInfo {
    pub signal: Signal,
    pub errno: i32,
    pub code: i32,
    pub detail: SignalDetail,

    /// Force delivery of the signal even if the target process has the signal blocked, and forces
    /// the default action for the signal.
    pub force: bool,

    /// The source code location where this signal was generated.
    source: SignalSource,

    /// The task that sent this signal. None for kernel-originated signals.
    sender: Option<Weak<Task>>,

    /// The trace id associated with this signal. Used to end the instaflow event when SignalInfo is
    /// dropped.
    trace_id: Option<fuchsia_trace::Id>,
}

impl PartialEq for SignalInfo {
    fn eq(&self, other: &Self) -> bool {
        self.signal == other.signal
            && self.errno == other.errno
            && self.code == other.code
            && self.detail == other.detail
            && self.force == other.force
            && self.source == other.source
            && match (&self.sender, &other.sender) {
                (Some(lhs), Some(rhs)) => Weak::ptr_eq(lhs, rhs),
                (None, None) => true,
                _ => false,
            }
            && self.trace_id == other.trace_id
    }
}

impl Eq for SignalInfo {}

impl Drop for SignalInfo {
    fn drop(&mut self) {
        if let Some(trace_id) = self.trace_id.take() {
            fuchsia_trace::instaflow_end!(
                TRACE_CATEGORY,
                "SignalFlow",
                self.signal.name(),
                trace_id
            );
        }
    }
}

impl Clone for SignalInfo {
    // Cloning a SignalInfo creates a new source, and a new trace id if tracing is enabled.
    #[track_caller]
    fn clone(&self) -> Self {
        Self::new(
            self.signal.clone(),
            self.errno,
            self.code,
            self.detail.clone(),
            self.force,
            self.sender.clone(),
        )
    }
}

macro_rules! make_siginfo {
            ($self:ident $(, $( $sifield:ident ).*, $value:expr)?) => {
                {
                struct_with_union_into_bytes!(uapi_path::siginfo_t {
                    __bindgen_anon_1.__bindgen_anon_1.si_signo: $self.signal.number() as i32,
                    __bindgen_anon_1.__bindgen_anon_1.si_errno: $self.errno,
                    __bindgen_anon_1.__bindgen_anon_1.si_code: $self.code,
                    $(
                        __bindgen_anon_1.__bindgen_anon_1._sifields.$( $sifield ).*: $value,
                    )?
                })
                }
            };
        }

macro_rules! signal_info_as_siginfo_bytes {
    ($ns:path, $self:ident) => {{
        use $ns as uapi_path;
        match $self.detail {
            SignalDetail::None => make_siginfo!($self),
            SignalDetail::Kill { ref pid, uid } => {
                make_siginfo!(
                    $self,
                    _kill,
                    uapi_path::__sifields__bindgen_ty_1 { _pid: pid.id, _uid: uid }
                )
            }
            SignalDetail::SIGCHLD { ref pid, uid, status } => make_siginfo!(
                $self,
                _sigchld,
                uapi_path::__sifields__bindgen_ty_4 {
                    _pid: pid.id,
                    _uid: uid,
                    _status: status,
                    ..Default::default()
                }
            ),
            SignalDetail::SigFault { addr } => {
                make_siginfo!(
                    $self,
                    _sigfault._addr,
                    uapi::uaddr { addr }.try_into().map_err(|_| errno!(EINVAL))?
                )
            }
            SignalDetail::SIGSYS { call_addr, syscall, arch } => make_siginfo!(
                $self,
                _sigsys,
                uapi_path::__sifields__bindgen_ty_7 {
                    _call_addr: uaddr::from(call_addr).try_into().map_err(|_| errno!(EINVAL))?,
                    _syscall: syscall as c_int,
                    _arch: arch as c_uint,
                }
            ),
            SignalDetail::Timer { ref timer } => {
                let sigval: uapi_path::sigval =
                    if timer.signal_event.notify == SignalEventNotify::None {
                        Default::default()
                    } else {
                        uapi::sigval::from(timer.signal_event.value)
                            .try_into()
                            .map_err(|_| errno!(EINVAL))?
                    };

                make_siginfo!(
                    $self,
                    _timer,
                    uapi_path::__sifields__bindgen_ty_2 {
                        _tid: timer.timer_id,
                        _overrun: timer.overrun_cur(),
                        _sigval: sigval,
                        ..Default::default()
                    }
                )
            }
            SignalDetail::Raw { data } => {
                let header = SignalInfoHeader {
                    signo: $self.signal.number(),
                    errno: $self.errno,
                    code: $self.code,
                };
                let mut array: [u8; SI_MAX_SIZE as usize] = [0; SI_MAX_SIZE as usize];
                let _ = header.write_to(&mut array[..SI_HEADER_SIZE]);
                array[SI_HEADER_SIZE..SI_MAX_SIZE as usize].copy_from_slice(&data);
                array
            }
        }
    }};
}

impl SignalInfo {
    /// Signal that originates from the Kernel.
    #[track_caller]
    pub fn kernel(signal: Signal) -> Self {
        Self::new(signal, 0, SI_KERNEL as i32, SignalDetail::None, false, None)
    }

    #[track_caller]
    pub fn forced(signal: Signal) -> Self {
        Self::new(signal, 0, SI_KERNEL as i32, SignalDetail::None, true, None)
    }

    #[track_caller]
    pub fn with_detail(signal: Signal, code: i32, detail: SignalDetail) -> Self {
        Self::new(signal, 0, code, detail, false, None)
    }

    #[track_caller]
    pub fn with_sender(
        signal: Signal,
        code: i32,
        detail: SignalDetail,
        sender: Option<Weak<Task>>,
    ) -> Self {
        Self::new(signal, 0, code, detail, false, sender)
    }

    #[track_caller]
    pub fn new(
        signal: Signal,
        errno: i32,
        code: i32,
        detail: SignalDetail,
        force: bool,
        sender: Option<Weak<Task>>,
    ) -> Self {
        static CACHE: trace_site_t = trace_site_t::new(0);
        let trace_id = TraceCategoryContext::acquire_cached(TRACE_CATEGORY, &CACHE).map(|_context| {
            let id = fuchsia_trace::Id::new();
            fuchsia_trace::instaflow_begin!(TRACE_CATEGORY, "SignalFlow", signal.name(), id, "code" => code);
            id
        });
        Self {
            signal,
            errno,
            code,
            detail,
            force,
            source: SignalSource::capture(),
            sender,
            trace_id,
        }
    }

    pub fn write<MA: MemoryAccessor>(
        &self,
        ma: &MA,
        addr: MultiArchUserRef<uapi::siginfo_t, uapi::arch32::siginfo_t>,
    ) -> Result<(), Errno> {
        if addr.is_arch32() {
            ma.write_memory(addr.addr(), &self.as_siginfo32_bytes()?)?;
        } else {
            ma.write_memory(addr.addr(), &self.as_siginfo64_bytes()?)?;
        }
        Ok(())
    }

    /// Zeroes out a `siginfo_t` at the given user address.
    pub fn zero<MA: MemoryAccessor>(
        ma: &MA,
        addr: MultiArchUserRef<uapi::siginfo_t, uapi::arch32::siginfo_t>,
    ) -> Result<(), Errno> {
        let size = if addr.is_arch32() {
            std::mem::size_of::<uapi::arch32::siginfo_t>()
        } else {
            std::mem::size_of::<uapi::siginfo_t>()
        };
        ma.zero(addr.addr(), size)?;
        Ok(())
    }

    pub fn as_siginfo_bytes(
        &self,
        arch_width: ArchWidth,
    ) -> Result<[u8; std::mem::size_of::<uapi::siginfo_t>()], Errno> {
        const_assert!(
            std::mem::size_of::<uapi::siginfo_t>()
                >= std::mem::size_of::<uapi::arch32::siginfo_t>()
        );
        if arch_width.is_arch32() {
            let mut result = [0_u8; std::mem::size_of::<uapi::siginfo_t>()];
            result[..std::mem::size_of::<uapi::arch32::siginfo_t>()]
                .copy_from_slice(&self.as_siginfo32_bytes()?);
            Ok(result)
        } else {
            self.as_siginfo64_bytes()
        }
    }

    /// Returns true if the signal was sent by the given task.
    pub fn is_sent_by(&self, weak_task: &Weak<Task>) -> bool {
        self.sender.as_ref().map_or(false, |sender| Weak::ptr_eq(sender, weak_task))
    }

    // TODO(tbodt): Add a bound requiring siginfo_t to be FromBytes. This will help ensure the
    // Linux side won't get an invalid siginfo_t.
    fn as_siginfo64_bytes(&self) -> Result<[u8; std::mem::size_of::<uapi::siginfo_t>()], Errno> {
        Ok(signal_info_as_siginfo_bytes!(uapi, self))
    }

    fn as_siginfo32_bytes(
        &self,
    ) -> Result<[u8; std::mem::size_of::<uapi::arch32::siginfo_t>()], Errno> {
        Ok(signal_info_as_siginfo_bytes!(uapi::arch32, self))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalDetail {
    None,
    Kill {
        pid: Pid,
        uid: uid_t,
    },
    SIGCHLD {
        pid: Pid,
        uid: uid_t,
        status: i32,
    },
    SigFault {
        addr: u64,
    },
    SIGSYS {
        call_addr: UserAddress,
        syscall: i32,
        arch: u32,
    },
    /// POSIX timer
    Timer {
        /// Timer where the signal comes from.
        ///
        /// Required fields in `uapi::siginfo_t` should be enquired from the timer only when needed.
        /// Because `overrun` counts might change when the signal is waiting in the queue.
        timer: IntervalTimerHandle,
    },
    Raw {
        data: [u8; SI_DETAIL_SIZE],
    },
}

impl Default for SignalDetail {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug)]
pub struct SignalEvent {
    pub value: SignalEventValue,
    pub signo: Option<Signal>,
    pub notify: SignalEventNotify,
}

impl SignalEvent {
    pub fn new(value: SignalEventValue, signo: Signal, notify: SignalEventNotify) -> Self {
        Self { value, signo: Some(signo), notify }
    }

    pub fn none() -> Self {
        Self { value: Default::default(), signo: None, notify: SignalEventNotify::None }
    }

    pub fn is_valid(&self, thread_group: &ThreadGroupReadGuard<'_>) -> bool {
        if self.notify != SignalEventNotify::None && self.signo.is_none() {
            return false;
        }

        if let SignalEventNotify::ThreadId(tid) = self.notify {
            if !thread_group.contains_task(tid) {
                return false;
            }
        }

        return true;
    }
}

impl TryFrom<sigevent> for SignalEvent {
    type Error = Errno;

    fn try_from(value: sigevent) -> Result<Self, Self::Error> {
        // SAFETY: _sigev_un was created with FromBytes so it's safe to access any variant
        // because all variants must be valid with all bit patterns.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let notify = match value.sigev_notify as u32 {
            SIGEV_SIGNAL => SignalEventNotify::Signal,
            SIGEV_NONE => SignalEventNotify::None,
            SIGEV_THREAD => unsafe {
                SignalEventNotify::Thread {
                    function: value._sigev_un._sigev_thread._function.into(),
                    attribute: value._sigev_un._sigev_thread._attribute.into(),
                }
            },
            SIGEV_THREAD_ID => SignalEventNotify::ThreadId(unsafe { value._sigev_un._tid }),
            _ => return error!(EINVAL),
        };

        Ok(match notify {
            SignalEventNotify::None => SignalEvent::none(),
            _ => SignalEvent::new(
                value.sigev_value.into(),
                UncheckedSignal::new(value.sigev_signo as u64).try_into()?,
                notify,
            ),
        })
    }
}

#[derive(Debug, PartialEq)]
/// Specifies how notification is to be performed.
pub enum SignalEventNotify {
    /// Notify the process by sending the signal specified in `SignalInfo::Signal`.
    Signal,
    /// Don't do anything when the event occurs.
    None,
    /// Notify the process by invoking the `function` as if it were the start function of
    /// a new thread.
    Thread { function: UserAddress, attribute: UserAddress },
    /// Similar to `SignalNotify::Signal`, but the signal is targeted at the thread ID.
    ThreadId(tid_t),
}

impl From<SignalEventNotify> for i32 {
    fn from(value: SignalEventNotify) -> Self {
        match value {
            SignalEventNotify::Signal => SIGEV_SIGNAL as i32,
            SignalEventNotify::None => SIGEV_NONE as i32,
            SignalEventNotify::Thread { .. } => SIGEV_THREAD as i32,
            SignalEventNotify::ThreadId(_) => SIGEV_THREAD_ID as i32,
        }
    }
}

/// Data passed with signal event notification.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct SignalEventValue(pub u64);

impl From<sigval_t> for SignalEventValue {
    fn from(value: sigval_t) -> Self {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        SignalEventValue(zerocopy::transmute!(unsafe { value._bindgen_opaque_blob }))
    }
}

impl From<SignalEventValue> for sigval_t {
    fn from(value: SignalEventValue) -> Self {
        Self { _bindgen_opaque_blob: zerocopy::transmute!(value.0) }
    }
}

#[cfg(not(feature = "debug_and_trace_logs_enabled"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSource {}

#[cfg(not(feature = "debug_and_trace_logs_enabled"))]
impl SignalSource {
    pub fn capture() -> Self {
        Self {}
    }
}

#[cfg(feature = "debug_and_trace_logs_enabled")]
#[derive(Clone, PartialEq, Eq)]
pub struct SignalSource {
    location: &'static std::panic::Location<'static>,
    task: String,
}

#[cfg(feature = "debug_and_trace_logs_enabled")]
impl std::fmt::Debug for SignalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalSource")
            .field("location", &format_args!("{}", self.location))
            .field("task", &self.task)
            .finish()
    }
}

#[cfg(feature = "debug_and_trace_logs_enabled")]
impl SignalSource {
    #[track_caller]
    pub fn capture() -> Self {
        let location = std::panic::Location::caller();
        let task = starnix_logging::with_current_task_info(|info| info.to_string());
        Self { location, task }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::task::PidEntry;
    use starnix_uapi::CLD_EXITED;
    use starnix_uapi::signals::{SIGCHLD, SIGPWR};

    #[::fuchsia::test]
    fn test_signal() {
        assert!(Signal::try_from(UncheckedSignal::from(0)).is_err());
        assert!(Signal::try_from(UncheckedSignal::from(1)).is_ok());
        assert!(Signal::try_from(UncheckedSignal::from(Signal::NUM_SIGNALS)).is_ok());
        assert!(Signal::try_from(UncheckedSignal::from(Signal::NUM_SIGNALS + 1)).is_err());
        assert!(!SIGCHLD.is_real_time());
        assert!(
            Signal::try_from(UncheckedSignal::from(uapi::SIGRTMIN + 12)).unwrap().is_real_time()
        );
        assert_eq!(format!("{SIGPWR}"), "SIGPWR(30)");
        assert_eq!(
            format!("{}", Signal::try_from(UncheckedSignal::from(uapi::SIGRTMIN + 10)).unwrap()),
            "SIGRTMIN+10(42)"
        );
    }

    #[::fuchsia::test]
    fn test_siginfo_bytes() {
        let mut sigchld_bytes =
            vec![17, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 123, 0, 0, 0, 200, 1, 0, 0, 2];
        sigchld_bytes.resize(std::mem::size_of::<uapi::siginfo_t>(), 0);
        assert_eq!(
            &SignalInfo::with_detail(
                SIGCHLD,
                CLD_EXITED as i32,
                SignalDetail::SIGCHLD { pid: PidEntry::new_for_test(123), uid: 456, status: 2 }
            )
            .as_siginfo64_bytes()
            .expect("as_siginfo_bytes"),
            sigchld_bytes.as_slice()
        );
    }
}
