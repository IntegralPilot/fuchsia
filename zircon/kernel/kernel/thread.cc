// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/**
 * @file
 * @brief  Kernel threading
 *
 * This file is the core kernel threading interface.
 *
 * @defgroup thread Threads
 * @{
 */
#include "kernel/thread.h"

#include <assert.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/arch/intrin.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/fxt/interned_string.h>
#include <lib/heap.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/ktrace.h>
#include <lib/lazy_init/lazy_init.h>
#include <lib/thread_sampler/thread_sampler.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <arch/debugger.h>
#include <arch/exception.h>
#include <arch/interrupt.h>
#include <arch/ops.h>
#include <arch/thread.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/cpu.h>
#include <kernel/dpc.h>
#include <kernel/idle_power_thread.h>
#include <kernel/lazy_owned_wait_queue.h>
#include <kernel/lockdep.h>
#include <kernel/mp.h>
#include <kernel/owned_wait_queue_pool.h>
#include <kernel/percpu.h>
#include <kernel/restricted.h>
#include <kernel/scheduler.h>
#include <kernel/stats.h>
#include <kernel/timer.h>
#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <lk/main.h>
#include <lockdep/lockdep.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/sampler_dispatcher.h>
#include <object/thread_dispatcher.h>
#include <pretty/hexdump.h>
#include <vm/kstack.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

// kernel counters.
// The counters below never decrease.
//
// counts the number of Threads successfully created.
KCOUNTER(thread_create_count, "thread.create")
// counts the number of detached Threads that exited. Never decreases.
KCOUNTER(thread_detached_exit_count, "thread.detached_exit")
// counts the number of Threads joined. Never decreases.
KCOUNTER(thread_join_count, "thread.join")
// counts the number of calls to suspend() that succeeded.
KCOUNTER(thread_suspend_count, "thread.suspend")
// counts the number of calls to resume() that succeeded.
KCOUNTER(thread_resume_count, "thread.resume")
// counts the number of times a thread's timeslice extension was activated (see
// |PreemptionState::SetTimesliceExtension|).
KCOUNTER(thread_timeslice_extended, "thread.timeslice_extended")
// counts the number of calls to restricted_kick() that succeeded.
KCOUNTER(thread_restricted_kick_count, "thread.restricted_kick")
// counts the number of failed samples
KCOUNTER(thread_sampling_failed, "thread.sampling_failed")
KCOUNTER(rseq_bad_ip, "rseq.bad_ip")

namespace {

// The global thread list.  This is a lazy_init type, since initial thread code
// manipulates the list before global constructors are run.  It's initialized
// by thread_init_early().
lazy_init::LazyInit<Thread::List> thread_list TA_GUARDED(Thread::get_list_lock());

// ConstructFirstThread() sets up the initial thread for each CPU from its
// running state.
//
// For the boot CPU, this must take care not to touch any global locks when
// invoked by early init code that runs before global ctors are called.  The
// thread_list is safe to mutate before global ctors are run.
//
// For secondary CPUs, this must use normal locking to guard both this Thread
// object and the thread_list.
//
// These two modes are represented by the template argument, which is a class
// of scoped guard objects that hold the necessary locks (or appear to).

class TA_SCOPED_CAP ConstructFirstThreadBootCpu {
 public:
  ConstructFirstThreadBootCpu() = delete;
  ConstructFirstThreadBootCpu(const ConstructFirstThreadBootCpu&) = delete;
  ConstructFirstThreadBootCpu operator=(const ConstructFirstThreadBootCpu&) = delete;

  // Appear to take all the locks that the secondary CPUs actually will.
  explicit ConstructFirstThreadBootCpu(Thread* t)
      TA_ACQ(Thread::get_list_lock(), chainlock_transaction_token, t->get_lock())
      : thread_{t} {}

  // This happens after the Thread has been fully initialized.
  ~ConstructFirstThreadBootCpu() TA_REL() { thread_list->push_front(thread_); }

 private:
  Thread* thread_;
};

class TA_SCOPED_CAP ConstructFirstThreadSecondaryCpu {
 public:
  ConstructFirstThreadSecondaryCpu() = delete;
  ConstructFirstThreadSecondaryCpu(const ConstructFirstThreadSecondaryCpu&) = delete;
  ConstructFirstThreadBootCpu& operator=(const ConstructFirstThreadBootCpu&) = delete;

  // The guard members actually hold all the necessary locks.
  explicit ConstructFirstThreadSecondaryCpu(Thread* t)
      TA_ACQ(Thread::get_list_lock(), chainlock_transaction_token, t->get_lock())
      : thread_{t},
        thread_guard_{NoIrqSaveOption, t->get_lock(), CLT_TAG("thread_construct_first")} {}

  // This happens after the Thread has been fully initialized, with all the
  // locks still held.
  ~ConstructFirstThreadSecondaryCpu() TA_REL() { thread_list->push_front(thread_); }

 private:
  Thread* thread_;
  Guard<SpinLock, IrqSave> list_guard_{&Thread::get_list_lock()};
  SingleChainLockGuard<ChainLockTransaction::StateOptions::NoIrqSave> thread_guard_;
};

template <class Guards>
void ConstructFirstThread(Thread* t, ktl::string_view name) {
  DEBUG_ASSERT(arch_ints_disabled());
  new (t) Thread(name);

  Guards guard{t};

  t->set_detached(true);

  // Set up the scheduler state.
  Scheduler::InitializeFirstThread(t);

  // Start out with preemption disabled to avoid attempts to reschedule until
  // threading is fully enabled.  This simplifies code paths shared between
  // initialization and runtime (e.g. logging).  Preemption is enabled when the
  // idle thread for the current CPU is ready.
  t->preemption_state().PreemptDisable();

  arch_thread_construct_first(t);
}

}  // namespace

Thread::MigrateList Thread::migrate_list_;

const char* ToString(enum thread_state state) {
  switch (state) {
    case THREAD_INITIAL:
      return "initial";
    case THREAD_READY:
      return "ready";
    case THREAD_RUNNING:
      return "running";
    case THREAD_BLOCKED:
      return "blocked";
    case THREAD_BLOCKED_READ_LOCK:
      return "blocked read lock";
    case THREAD_SLEEPING:
      return "sleeping";
    case THREAD_SUSPENDED:
      return "suspended";
    case THREAD_DEATH:
      return "death";
    default:
      return "[unknown]";
  }
}

void WaitQueueCollection::ThreadState::Block(Thread* const current_thread,
                                             Interruptible interruptible, zx_status_t status) {
  blocked_status_ = status;
  interruptible_ = interruptible;
  Scheduler::Block(current_thread);
  interruptible_ = Interruptible::No;
}

void WaitQueueCollection::ThreadState::Unsleep(Thread* thread, zx_status_t status) {
  blocked_status_ = status;
  Scheduler::Unblock(thread);
}

WaitQueueCollection::ThreadState::~ThreadState() { DEBUG_ASSERT(blocking_wait_queue_ == nullptr); }

// Default constructor/destructor.
Thread::Thread() = default;

Thread::~Thread() {
  // At this point, the thread must not be on the global thread list or migrate
  // list.
  DEBUG_ASSERT(!thread_list_node_.InContainer());
  DEBUG_ASSERT(!migrate_list_node_.InContainer());

  // owned_wait_queues_ is a fbl:: list of unmanaged pointers.  It will debug
  // assert if it is not empty when it destructs; we do not need to do so
  // here.
}

void Thread::set_name(ktl::string_view name) {
  // |name| must fit in ZX_MAX_NAME_LEN bytes, minus 1 for the trailing NUL.
  name = name.substr(0, ZX_MAX_NAME_LEN - 1);
  memcpy(name_, name.data(), name.size());
  memset(name_ + name.size(), 0, ZX_MAX_NAME_LEN - name.size());
}

Thread::Thread(ktl::string_view name) {
  set_name(name);

#if WITH_LOCK_DEP
  lockdep::SystemInitThreadLockState(&lock_state());
#endif
}

void TaskState::Init(thread_start_routine entry, void* arg) {
  entry_ = entry;
  arg_ = arg;
}

zx_status_t TaskState::Join(Thread* const current_thread, zx_instant_mono_t deadline) {
  return retcode_wait_queue_.Block(current_thread, deadline, Interruptible::No);
}

bool TaskState::TryWakeJoiners(zx_status_t status) {
  ktl::optional<uint32_t> result;

  // Attempt to lock the queue first.
  if (retcode_wait_queue_.get_lock().AcquireOrBackoff()) {
    // Now, try to lock and wake all of our waiting threads.
    result = retcode_wait_queue_.WakeAllLocked(status);

    // No matter what just happened, we need to drop the queue's lock.
    retcode_wait_queue_.get_lock().Release();
  }

  // If we got a result, then we succeeded in waking the threads we wanted to
  // wake.
  return result != ktl::nullopt;
}

static void free_thread_resources(Thread* t) {
  // Free the thread structure itself unless marked otherwise.  If not freeing
  // it, manually trigger the Thread destructor so that DEBUG_ASSERTs present
  // in the owned_wait_queues member get triggered in all cases.
  OwnedWaitQueuePool::Get().Shrink();
  if (t->free_struct()) {
    delete t;
  } else {
    t->~Thread();
  }
}

template <typename F>
zx_status_t Thread::Current::Fault(vaddr_t va, F resolve_fault) {
  if (is_kernel_address(va)) {
    // Kernel addresses should never fault.
    return ZX_ERR_NOT_FOUND;
  }

  // If this thread is a kernel thread, then it must be running a unit test that set `aspace_`
  // explicitly, so use `aspace_` to resolve the fault.
  //
  // If this is a user thread in restricted mode, then `aspace_` is set to the restricted address
  // space and should be used to resolve the fault.
  //
  // Otherwise, this is a user thread running in normal mode. Therefore, we must consult the
  // process' aspace_at function to resolve the fault.
  Thread* t = Thread::Current::Get();
  VmAspace* containing_aspace;
  if (!t->user_thread_ || t->in_restricted()) {
    containing_aspace = t->aspace_;
  } else {
    containing_aspace = t->user_thread_->process()->aspace_at(va);
  }

  // Call the appropriate fault function on the containing address space.
  return resolve_fault(containing_aspace);
}

zx_status_t Thread::Current::PageFault(vaddr_t va, uint flags) {
  return Fault(va, [va, flags](VmAspace* aspace) { return aspace->PageFault(va, flags); });
}

zx_status_t Thread::Current::SoftFault(vaddr_t va, uint flags) {
  return Fault(va, [va, flags](VmAspace* aspace) { return aspace->SoftFault(va, flags); });
}

zx_status_t Thread::Current::SoftFaultInRange(vaddr_t va, uint flags, size_t len) {
  return Fault(
      va, [va, flags, len](VmAspace* aspace) { return aspace->SoftFaultInRange(va, flags, len); });
}

zx_status_t Thread::Current::AccessedFault(vaddr_t va) {
  return Fault(va, [va](VmAspace* aspace) { return aspace->AccessedFault(va); });
}

void Thread::Trampoline() {
  // Handle the special case of starting a new thread for the first time,
  // releasing the previous thread's lock as well as the current thread's lock
  // in the process, before restoring interrupts and dropping into the thread's
  // entry point.
  Scheduler::TrampolineLockHandoff();
  arch_enable_ints();

  // Call into the thread's entry point, and call exit when finished.
  const TaskState& task_state = Thread::Current::Get()->task_state_;
  int ret = task_state.entry()(task_state.arg());
  Thread::Current::Exit(ret);
}

/**
 * @brief  Create a new thread
 *
 * This function creates a new thread.  The thread is initially suspended, so you
 * need to call thread_resume() to execute it.
 *
 * @param  t               If not nullptr, use the supplied Thread
 * @param  name            Name of thread
 * @param  entry           Entry point of thread
 * @param  arg             Arbitrary argument passed to entry(). It can be null.
 *                         in which case |user_thread| will be used.
 * @param  priority        Execution priority for the thread.
 * @param  alt_trampoline  If not nullptr, an alternate trampoline for the thread
 *                         to start on.
 *
 * Thread priority is an integer from 0 (lowest) to 31 (highest).  Some standard
 * priorities are defined in <kernel/thread.h>:
 *
 *  HIGHEST_PRIORITY
 *  DPC_PRIORITY
 *  HIGH_PRIORITY
 *  DEFAULT_PRIORITY
 *  LOW_PRIORITY
 *  IDLE_PRIORITY
 *  LOWEST_PRIORITY
 *
 * @return  Pointer to thread object, or nullptr on failure.
 */
Thread* Thread::CreateEtc(Thread* t, ktl::string_view name, thread_start_routine entry, void* arg,
                          const SchedulerState::BaseProfile& profile,
                          thread_trampoline_routine alt_trampoline) {
  // Ensure pool capacity for this thread. Do this before allocating
  // the thread itself to avoid complex unwinding.
  zx_status_t pool_status = OwnedWaitQueuePool::Get().Grow();
  if (pool_status != ZX_OK) {
    return nullptr;
  }

  unsigned int flags = 0;

  if (t) {
    // assert that t is at least as aligned as the Thread is supposed to be
    DEBUG_ASSERT(IS_ROUNDED(t, alignof(Thread)));
    new (t) Thread(name);
  } else {
    flags |= THREAD_FLAG_FREE_STRUCT;
    fbl::AllocChecker ac;
    t = new (ac) Thread(name);
    if (!ac.check()) {
      OwnedWaitQueuePool::Get().Shrink();
      return nullptr;
    }
  }

  {
    // Do not hold the thread's lock while we initialize it.  We are going to
    // perform some operations (like dynamic memory allocation) which will
    // require us to obtain potentially contested mutexes, something we cannot
    // safely do while holding any spinlocks, or chainlocks.
    //
    // Not holding the thread's lock here _should_ be OK.  We are basically in
    // the process of constructing the thread, no one else knows about this
    // memory just yet.  There are a limited number of ways that other CPUs in
    // the system could find out about this thread, and they should all involve
    // synchronizing via a lock which should establish proper memory ordering
    // semantics.
    //
    // 1) After we add the thread to the global thread list, debug code could
    //    find the thread, but only after synchronizing via the global thread
    //    list lock.
    // 2) After we add the thread to the scheduler (after CreateEtc), other CPUs
    //    can find out about it, but only after synchronizing via the
    //    scheduler's queue lock.
    //
    struct FakeChainLockTransaction {
      // Lie to the analyzer about the fact that we are in a CLT, so we can lie
      // to the analyzer about holding the thread's lock.
      FakeChainLockTransaction() TA_ACQ(chainlock_transaction_token) {}
      ~FakeChainLockTransaction() TA_REL(chainlock_transaction_token) {}
    } fake_clt{};
    t->get_lock().MarkNeedsRelease();  // This is a lie, we don't hold the lock.  See above.

    t->task_state_.Init(entry, arg);
    Scheduler::InitializeThread(t, profile);

    zx_status_t status = t->stack_.Init();
    if (status != ZX_OK) {
      t->get_lock().MarkReleased();
      free_thread_resources(t);
      return nullptr;
    }

    // save whether or not we need to free the thread struct and/or stack
    t->flags_ = flags;

    if (likely(alt_trampoline == nullptr)) {
      alt_trampoline = &Thread::Trampoline;
    }

    // set up the initial stack frame
    arch_thread_initialize(t, (vaddr_t)alt_trampoline);

    t->get_lock().MarkReleased();
  }

  // Add it to the global thread list.  Be sure to grab the locks in the proper
  // order; global thread list lock first, then the thread's lock second.
  {
    Guard<SpinLock, IrqSave> list_guard{&list_lock_};
    SingleChainLockGuard thread_guard{NoIrqSaveOption, t->get_lock(), CLT_TAG("Thread::CreateEtc")};
    thread_list->push_front(t);
  }

  kcounter_add(thread_create_count, 1);
  return t;
}

Thread* Thread::Create(ktl::string_view name, thread_start_routine entry, void* arg, int priority) {
  return Thread::CreateEtc(nullptr, name, entry, arg, SchedulerState::BaseProfile{priority},
                           nullptr);
}

Thread* Thread::Create(ktl::string_view name, thread_start_routine entry, void* arg,
                       const SchedulerState::BaseProfile& profile) {
  return Thread::CreateEtc(nullptr, name, entry, arg, profile, nullptr);
}

/**
 * @brief  Make a suspended thread executable.
 *
 * This function is called to start a thread which has just been
 * created with thread_create() or which has been suspended with
 * thread_suspend(). It can not fail.
 */
void Thread::Resume() {
  canary_.Assert();

  // We cannot allow a resume to happen if we are holding any spinlocks, unless
  // local preemption has been disabled.  If we have local preemption enabled,
  // and a spinlock held, then it is theoretically possible for our current CPU
  // to be chosen as the target for the thread being resumed, triggering a local
  // preemption event.  This is illegal; being preempted while holding a
  // spinlock means that we might lose our CPU while holding a spinlock.
  //
  // So, assert this here.  Either we have no spinlocks, or preemption has
  // already been disabled (presumably to the point where we have dropped all of
  // our spinlocks)
  DEBUG_ASSERT_MSG((arch_num_spinlocks_held() == 0) ||
                       (Thread::Current::Get()->preemption_state().PreemptIsEnabled() == false),
                   "It is illegal to Resume a thread when any spinlocks are held unless local "
                   "preemption is disabled.  (spinlocks held %u, preemption enabled %d)",
                   arch_num_spinlocks_held(),
                   Thread::Current::Get()->preemption_state().PreemptIsEnabled());

  // Disable preemption before grabbing the target thread's lock.  This will
  // ensure that we have dropped the target thread's lock before any local
  // reschedule takes place, helping to avoid lock thrash as the target thread
  // becomes scheduled for the first time.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    get_lock().AcquireFirstInChain();
    ChainLockTransaction::Finalize();

    if (state() == THREAD_DEATH) {
      // The thread is dead, resuming it is a no-op.
      get_lock().Release();
      return ChainLockTransaction::Done;
    }

    // Emit the thread metadata the first time the thread is resumed so that trace
    // events written by this thread have the correct name and process association.
    if (state() == THREAD_INITIAL) {
      const zx_koid_t job_koid = user_thread() ? user_thread()->process()->job()->get_koid() : 0u;
      KTRACE_KERNEL_OBJECT("kernel:meta", tid(), ZX_OBJ_TYPE_THREAD, name(),
                           ("process", ktrace::Koid(pid())), ("job", ktrace::Koid(job_koid)));
    }

    // Clear the suspend signal in case there is a pending suspend
    clear_signals(THREAD_SIGNAL_SUSPEND);
    if (state() == THREAD_INITIAL || state() == THREAD_SUSPENDED) {
      // Wake up the new thread, putting it in a run queue on a cpu.
      Scheduler::Unblock(this);
    } else {
      get_lock().Release();
    }

    kcounter_add(thread_resume_count, 1);
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption, CLT_TAG("Thread::Resume"),
                                  do_transaction);
}

zx_status_t Thread::DetachAndResume() {
  zx_status_t status = Detach();
  if (status != ZX_OK) {
    return status;
  }
  Resume();
  return ZX_OK;
}

/**
 * @brief  Suspend or Kill an initialized/ready/running thread
 *
 * @return ZX_OK on success, ZX_ERR_BAD_STATE if the thread is dead
 */
zx_status_t Thread::SuspendOrKillInternal(SuspendOrKillOp op) {
  canary_.Assert();
  DEBUG_ASSERT(!IsIdle());

  const bool suspend_op = op == SuspendOrKillOp::Suspend;
  const zx_status_t wakeup_status =
      suspend_op ? ZX_ERR_INTERNAL_INTR_RETRY : ZX_ERR_INTERNAL_INTR_KILLED;
  auto set_signals = fit::defer([this, suspend_op]() {
    if (suspend_op) {
      this->set_signals(THREAD_SIGNAL_SUSPEND);
      kcounter_add(thread_suspend_count, 1);
    } else {
      this->set_signals(THREAD_SIGNAL_KILL);
    }
  });

  // Disable preemption to defer rescheduling until the transaction is completed.
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token,
                   preempt_disabled_token) -> ChainLockTransaction::Result<zx_status_t> {
    ChainLockGuard thread_guard{get_lock()};
    const thread_state cur_state = state();

    // If this is a kill operation, and we are killing ourself, just set our
    // signal bits and get out.  We will process the pending signal (eventually)
    // as we unwind.
    if ((suspend_op == false) && (this == Thread::Current::Get())) {
      DEBUG_ASSERT(cur_state == THREAD_RUNNING);
      set_signals.call();
      return ZX_OK;
    }

    // If the thread is in the DEATH state, it cannot be suspended.  Do not set
    // any signals, or increment any kcounters on our way out.
    if (cur_state == THREAD_DEATH) {
      set_signals.cancel();
      return ZX_ERR_BAD_STATE;
    }

    // If the thread is sleeping, or it is blocked, then we can only wake it up
    // for the kill or suspend operation if it is currently interruptible.
    // Check this; if the thread is not interruptible, simply set the signals
    // and get out. Someday, when the thread finally unblocks, it can deal with
    // the signals.
    const bool is_blocked =
        (cur_state == THREAD_BLOCKED) || (cur_state == THREAD_BLOCKED_READ_LOCK);
    if ((is_blocked || (cur_state == THREAD_SLEEPING)) &&
        (wait_queue_state_.interruptible() == Interruptible::No)) {
      set_signals.call();
      return ZX_OK;
    }

    // Unblocking a thread who is blocked in a wait queue requires that we hold
    // the entire PI chain starting from the thread before proceeding.
    // Attempting to obtain this chain of locks might result in a conflict which
    // requires us to back off, releasing all locks (including the initial
    // thread's) before trying again.
    WaitQueueBase* const wq = wait_queue_state_.blocking_wait_queue();
    if (is_blocked && OwnedWaitQueue::LockPiChain(*this) == ChainLock::Result::Backoff) {
      return ChainLockTransaction::Action::Backoff;
    }

    // OK, we have now obtained all of our locks and are committed to the
    // operation.  We can go ahead and set our signal state and increment our
    // kcounters now.
    ChainLockTransaction::Finalize();
    set_signals.call();

    switch (cur_state) {
      case THREAD_INITIAL:
        // TODO(johngro); Figure out what to do about this.  The original
        // versions of suspend and kill seem to disagree about whether or not it
        // is safe to be suspending or killing a thread at this point.  The
        // original suspend had the comment below, indicating that it should be
        // OK to unblock the thread. The Kill code, however, had this comment:
        //
        // ```
        // thread hasn't been started yet.
        // not really safe to wake it up, since it's only in this state because it's under
        // construction by the creator thread.
        // ```
        //
        // indicating that the thread was not yet safe to wake.  It really feels
        // like this should be an ASSERT (people should not be
        // suspending/killing threads in the initial state), or it should just
        // leave the thread as is.  The signal is set, and will be handled when
        // the thread ends up running for the first time.

        // Thread hasn't been started yet, add it to the run queue to transition
        // properly through the INITIAL -> READY state machine first, then it
        // will see the signal and go to SUSPEND before running user code.
        //
        // Though the state here is still INITIAL, the higher-level code has
        // already executed ThreadDispatcher::Start() so all the userspace
        // entry data has been initialized and will be ready to go as soon as
        // the thread is unsuspended.
        thread_guard.Unguard();
        Scheduler::Unblock(this);
        return ZX_OK;
      case THREAD_READY:
        // thread is ready to run and not blocked or suspended.
        // will wake up and deal with the signal soon.
        break;
      case THREAD_RUNNING:
        // thread is running (on another cpu)
        // The following call is not essential.  It just makes the
        // thread suspension/kill happen sooner rather than at the next
        // timer interrupt or syscall.
        mp_interrupt(mp_ipi_target::MASK, cpu_num_to_mask(scheduler_state_.curr_cpu_));
        break;
      case THREAD_BLOCKED:
      case THREAD_BLOCKED_READ_LOCK:
        // thread is blocked on something and marked interruptible.  We are
        // holding the entire PI chain at this point, and calling UnblockThread
        // on our blocking wait queue will release then entire PI chain for us
        // as part of the unblock operation.
        wq->get_lock().AssertAcquired();
        thread_guard.Unguard();
        wq->UnblockThread(this, wakeup_status);
        return ZX_OK;
      case THREAD_SLEEPING:
        // thread is sleeping and interruptible
        thread_guard.Unguard();
        wait_queue_state_.Unsleep(this, wakeup_status);
        return ZX_OK;
      case THREAD_SUSPENDED:
        // If this is a kill operation, and the thread is suspended, resume it
        // so it can get the kill signal.  Otherwise, do nothing.  The thread
        // was already suspended, and it still is.
        if (suspend_op == false) {
          thread_guard.Unguard();
          Scheduler::Unblock(this);
          return ZX_OK;
        }
        break;
      default:
        panic("Unexpected thread state %u", cur_state);
    }

    return ZX_OK;
  };

  return ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                         CLT_TAG("Thread::SuspendOrKillInternal"), do_transaction);
}

zx_status_t Thread::RestrictedKick() {
  LTRACE_ENTRY;

  canary_.Assert();
  DEBUG_ASSERT(!IsIdle());

  bool kicking_myself = (Thread::Current::Get() == this);

  // Hold the thread's lock while we examine its state and figure out what to
  // do.
  cpu_mask_t ipi_mask{0};
  {
    SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::RestrictedKick")};

    if (state() == THREAD_DEATH) {
      return ZX_ERR_BAD_STATE;
    }

    set_signals(THREAD_SIGNAL_RESTRICTED_KICK);

    if (state() == THREAD_RUNNING && !kicking_myself) {
      // thread is running (on another cpu)
      // Send an IPI after dropping this thread's lock to make sure that the
      // thread processes the new signals. If the thread executes a regular
      // syscall or is rescheduled the signals will also be processed then, but
      // there's no upper bound on how long that could take.
      //
      // TODO(johngro) - what about a thread which is in a transient state
      // (active migration or being stolen).  Should we skip the IPI and just
      // make sure to force the scheduler to always re-evaluate pending signals
      // after exiting a transient state?
      ipi_mask = cpu_num_to_mask(scheduler_state_.curr_cpu_);
    }
  }

  if (ipi_mask != 0) {
    mp_interrupt(mp_ipi_target::MASK, ipi_mask);
  }

  kcounter_add(thread_restricted_kick_count, 1);
  return ZX_OK;
}

// Signal an exception on the current thread, to be handled when the
// current syscall exits.  Unlike other signals, this is synchronous, in
// the sense that a thread signals itself.  This exists primarily so that
// we can unwind the stack in order to get the state of userland's
// callee-saved registers at the point where userland invoked the
// syscall.
void Thread::Current::SignalPolicyException(uint32_t policy_exception_code,
                                            uint32_t policy_exception_data) {
  Thread* t = Thread::Current::Get();
  SingleChainLockGuard guard{IrqSaveOption, t->get_lock(),
                             CLT_TAG("Thread::Current::SignalPolicyException")};
  t->set_signals(THREAD_SIGNAL_POLICY_EXCEPTION);
  t->extra_policy_exception_code_ = policy_exception_code;
  t->extra_policy_exception_data_ = policy_exception_data;
}

void Thread::Current::SignalPolicyKill() {
  Thread* t = Thread::Current::Get();
  SingleChainLockGuard guard{IrqSaveOption, t->get_lock(),
                             CLT_TAG("Thread::Current::SignalPolicyKill")};
  t->set_signals(THREAD_SIGNAL_POLICY_KILL);
}

void Thread::EraseFromListsLocked() {
  thread_list->erase(*this);
  if (migrate_list_node_.InContainer()) {
    migrate_list_.erase(*this);
  }
}

zx_status_t Thread::Join(int* out_retcode, zx_instant_mono_t deadline) {
  canary_.Assert();

  // No thread should be attempting to join itself.
  Thread* const current_thread = Thread::Current::Get();
  DEBUG_ASSERT(this != current_thread);

  for (bool finished = false; !finished;) {
    const auto do_transaction =
        [&]() TA_REQ(chainlock_transaction_token) -> ChainLockTransaction::Result<zx_status_t> {
      // Start by locking the global thread list lock, and the target thread's lock.
      Guard<SpinLock, NoIrqSave> list_guard{&list_lock_};
      ChainLockGuard guard{get_lock()};

      if (flags_ & THREAD_FLAG_DETACHED) {
        // the thread is detached, go ahead and exit
        return ZX_ERR_BAD_STATE;
      }

      // If we need to wait for our thread to die, we need exchange our current
      // locks (the target thread's lock and the list lock) for the current thread
      // and the wait_queue's lock.  If this operation fails with a back-off
      // error, we need to drop all of our locks and try again.
      if (state() != THREAD_DEATH) {
        if (!AcquireBothOrBackoff(current_thread->get_lock(), task_state_.get_lock())) {
          return ChainLockTransaction::Action::Backoff;
        }

        // We now have all of the locks we need for this operation.
        ChainLockTransaction::Finalize();

        // Now go ahead and drop our other locks, and block in the wait queue.
        // This will release the wait queue's lock for us, and will release and
        // re-acquire our thread's lock (as we block and eventually become
        // rescheduled).  Once we are done waiting, drop the current thread's
        // lock, then either start again, or propagate any wait error we encounter
        // up the stack.
        guard.Release();
        list_guard.Release();
        const zx_status_t status = task_state_.Join(current_thread, deadline);
        current_thread->get_lock().Release();
        if (status != ZX_OK) {
          return status;
        }

        // break out of the lock-acquisition loop, but do not set the finished
        // flag.  This way, we will start a ChainLockTransaction on the new
        // check of the thread's state instead of attempting to use our already
        // finalized transaction.
        return ZX_ERR_SHOULD_WAIT;
      }

      canary_.Assert();
      DEBUG_ASSERT(state() == THREAD_DEATH);
      wait_queue_state_.AssertNotBlocked();

      // save the return code
      if (out_retcode) {
        *out_retcode = task_state_.retcode();
      }

      // remove it from global lists
      EraseFromListsLocked();

      // Our canary_ will be cleared out in free_thread_resources, which
      // explicitly invokes ~Thread.  Set the finished flag and break out of our
      // retry loop, dropping our locks in the process.
      finished = true;
      return ZX_ERR_SHOULD_WAIT;
    };
    const zx_status_t result =
        ChainLockTransaction::UntilDone(IrqSaveOption, CLT_TAG("Thread::Join"), do_transaction);
    if (result != ZX_ERR_SHOULD_WAIT) {
      return result;
    }
  }

  free_thread_resources(this);

  kcounter_add(thread_join_count, 1);

  return ZX_OK;
}

zx_status_t Thread::Detach() {
  canary_.Assert();

  enum class JoinThread : bool {
    No,
    Yes,
  };

  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token,
                   preempt_disabled_token) -> ChainLockTransaction::Result<JoinThread> {
    // Lock the target thread's state before proceeding.
    ChainLockGuard guard{get_lock()};

    // Try to wake our joiners with the error "BAD_STATE" (the thread is now
    // detached and can no longer be joined).  If we fail, we need to drop our
    // locks and try again.  If we succeed, our transaction will be finalized
    // and we can finish updating all of our state.
    if (task_state_.TryWakeJoiners(ZX_ERR_BAD_STATE) == false) {
      return ChainLockTransaction::Action::Backoff;
    }

    // If the target thread is not yet dead, flag it as detached and get out.
    if (state() != THREAD_DEATH) {
      flags_ |= THREAD_FLAG_DETACHED;
      return JoinThread::No;
    }

    // Looks like the thread has already died.  Make sure the DETACHED flag has
    // been cleared (so that Join continues).
    flags_ &= ~THREAD_FLAG_DETACHED;  // makes sure Join continues
    // Drop our lock and Join to finish the cleanup.
    return JoinThread::Yes;
  };

  JoinThread result = ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                                      CLT_TAG("Thread::Detach"), do_transaction);

  if (result == JoinThread::Yes) {
    // The thread was already dead, meaning we are the only ones positioned to do the cleanup.
    //
    // This Thread::Join cannot be moved inside do_transaction, as it creates its own
    // ChainLockTransaction, and nested transactions are not allowed.
    return Join(nullptr, 0);
  }
  return ZX_OK;
}

// called back in the DPC worker thread to free the stack and/or the thread structure
// itself for a thread that is exiting on its own.
void Thread::FreeDpc(Dpc* dpc) {
  Thread* t = dpc->arg<Thread>();

  t->canary_.Assert();

  // grab and release the thread's lock, which effectively serializes us with
  // the thread that is queuing itself for destruction.  It ensures that:
  //
  // 1) The thread is no longer on any lists or referenced in any way.
  // 2) The thread is no longer assigned to any scheduler, and it has already
  //    entered the scheduler for the final time.
  {
    SingleChainLockGuard guard{IrqSaveOption, t->get_lock(), CLT_TAG("Thread::FreeDpc")};
    DEBUG_ASSERT(t->state() == THREAD_DEATH);
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
  }

  free_thread_resources(t);
}

/**
 * @brief Remove this thread from the scheduler, discarding
 * its execution state.
 *
 * This is almost certainly not the function you want.  In the general case,
 * this is incredibly unsafe.
 *
 * This will free any resources allocated by thread_create.
 */
void Thread::Forget() {
  DEBUG_ASSERT(Thread::Current::Get() != this);

  {
    Guard<SpinLock, IrqSave> list_guard{&list_lock_};
    SingleChainLockGuard guard{NoIrqSaveOption, get_lock(), CLT_TAG("Thread::Forget")};
    DEBUG_ASSERT(!wait_queue_state_.InWaitQueue());
    EraseFromListsLocked();
  }

  free_thread_resources(this);
}

/**
 * @brief  Terminate the current thread
 *
 * Current thread exits with the specified return code.
 *
 * This function does not return.
 */
__NO_RETURN void Thread::Current::Exit(int retcode) {
  Thread* const current_thread = Thread::Current::Get();
  current_thread->canary_.Assert();
  DEBUG_ASSERT(!current_thread->IsIdle());

  if (current_thread->user_thread_) {
    DEBUG_ASSERT(!arch_ints_disabled());
    current_thread->user_thread_->ExitingCurrent();
  }

  // Start by locking this thread, and setting the flag which indicates that it
  // can no longer be the owner of any OwnedWaitQueues in the system.  This will
  // ensure that the OWQ code no longer allows the thread to become the owner of
  // any queues.
  //
  AnnotatedAutoPreemptDisabler aapd;
  InterruptDisableGuard irqd;
  {
    SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("Thread::Current::Exit deny WQ ownership")};
    current_thread->can_own_wait_queues_ = false;
  }

  // Now that that we can no longer own queues, we can disown any queues that we
  // currently happen to own.
  OwnedWaitQueue::DisownAllQueues(current_thread);

  // Now, to complete the exit operation, we must be holding this thread's lock in
  // addition to the global thread list lock.
  bool queue_free_dpc{false};
  {
    Guard<SpinLock, NoIrqSave> list_guard{&list_lock_};
    SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("Thread::Current::Exit finish in list_guard")};

    // enter the dead state and finish any migration.
    Scheduler::RunInLockedCurrentScheduler([current_thread](Scheduler&) {
      ChainLockTransaction::AssertActive();
      current_thread->get_lock().AssertHeld();
      current_thread->set_death();
    });
    current_thread->task_state_.set_retcode(retcode);
    current_thread->CallMigrateFnLocked(Thread::MigrateStage::Exiting);

    // if we're detached, then do our teardown here
    if (current_thread->flags_ & THREAD_FLAG_DETACHED) {
      kcounter_add(thread_detached_exit_count, 1);

      // remove it from global lists and drop the global list lock.
      current_thread->EraseFromListsLocked();
      list_guard.Release();

      // Remember to queue a dpc to free the stack and, optionally, the thread
      // structure
      //
      // We are detached and will need to queue a DPC in order to clean up our
      // thread structure and stack after we are gone.  Queueing a DPC will
      // involve signaling the DPC thread, so we (technically) need to drop our
      // thread's lock and end our current chain lock transaction  before doing
      // so.
      //
      // Preemption is disabled, interrupts are off, and DPC threads always run
      // on the CPU they were queued from.  Because of this, we can be sure that
      // the DPC thread is not going to preempt us and free our stack out from
      // under us before we manage to complete our final reschedule and context
      // switch.
      //
      // TODO(johngro): I cannot think of a way where waking up a DPC thread
      // should result in us deadlocking in a lock cycle.  Logically, our
      // current thread is no longer visible to anyone.
      //
      // 1) We are the currently running thread in a scheduler which cannot
      //    currently reschedule (until we drop into our final reschedule).
      // 2) We are not in any wait queues.
      // 3) We have become disconnected from our ThreadDispatcher and all
      //    handles referring to it.
      //
      // Waking the DPC thread would involve holding the DPC thread's lock and
      // its wait queues lock in order to remove the thread from the queue,
      // followed by holding our schedulers lock to insert the DPC thread.
      //
      // In _theory_ it should be OK to use our existing ChainLockTransaction to
      // queue the DPC while holding our thread's lock.  If we can prove that
      // this is safe, it may be a good idea to come back here later and queue
      // the DPC while holding the thread's lock, just to simplify the structure
      // of the code around here.
      if (current_thread->stack_.base() || (current_thread->flags_ & THREAD_FLAG_FREE_STRUCT)) {
        queue_free_dpc = true;
      }
    } else {
      // Looks like we didn't need the global list lock after all.  Drop it and
      // signal anyone who is waiting to join us.
      list_guard.Release();

      // Our thread is both running, and exiting.  It is not a member of any
      // wait queue, and cannot be rescheduled (preemption is disabled).  It
      // should not be possible for it to be involved in any lock cycles, so we
      // are free to keep trying to to wake our joiners until we finally
      // succeed.
      //
      // Do not drop our thread's lock here, we *must* hold it until after the
      // final reschedule is complete.  Failure to do this means that a joiner
      // thread might wake up and free our stack out from under us before our
      // final reschedule.
      //
      // TODO(johngro): TryWakeJoiners may need to lock a wait queue and more
      // than one thread in order to wake the current set of joiners.  We have
      // already finalized our transaction at this point, and are still holding
      // a lock.  Bending the rules here requires that we do something we really
      // don't want to be doing, which is to undo the Finalized state of the
      // transaction while we are still holding a lock.
      //
      // It would be *much* cleaner if we didn't have to do this.  We would
      // rather be in a position to drop our thread's lock and allow an
      // unconditional wake of our joiners instead.
      //
      // So, look introducing a new lock (the thread cleanup spinlock?) which
      // could provide us the protection we would need.  The idea would be:
      //
      // 1) Earlier in this function, obtain this new cleanup lock.
      // 2) When we get to here, drop the thread's lock for the wakeup
      //    operation.
      // 3) Once wakeup is complete, re-obtain the thread's lock.
      // 4) Drop the cleanup lock.
      // 5) Enter the final reschedule.
      //
      // On the joiner/cleanup side of things, the last joiner out of the door
      // would just need to:
      //
      // 1) Obtain the cleanup lock.
      // 2) Obtain the thread's lock.
      // 3) Drop all locks.  We have now synced up with everyone else and should
      //    be free to cleanup the memory now.
      ChainLockTransaction& active_clt = ChainLockTransaction::ActiveRef();
      active_clt.Restart(CLT_TAG("Thread::Current::Exit wake joiners"));
      while (current_thread->task_state_.TryWakeJoiners(ZX_OK) == false) {
        // Note, we are still holding the current thread's lock, so we cannot
        // fully relax the active CLT (which would demand that we hold zero
        // locks).  See the note above about potentially finding a way to be
        // able to drop the current thread's lock here.
        arch::Yield();
      }
    }

    if (!queue_free_dpc) {
      // Drop into the final reschedule while still holding our lock.  There
      // should be no coming back from this.  We may have gotten here because we
      // were still joinable, or because we were detached but had a statically
      // allocated stack/thread-struct, but either way, we don't need to drop and
      // re-acquire our thread's lock in order to finish exiting at this point.
      Scheduler::RescheduleInternal(current_thread);
      panic("somehow fell through thread_exit()\n");
    }
  }

  // It should not be possible to get to this point without needing to drop our
  // locks and queue a DPC to clean ourselves up.
  ASSERT(queue_free_dpc);
  Dpc free_dpc(&Thread::FreeDpc, current_thread);
  [[maybe_unused]] zx_status_t status = DpcRunner::Enqueue(free_dpc);
  DEBUG_ASSERT(status == ZX_OK);

  // Relock our thread one last time before dropping into our final reschedule
  // operation.
  SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                             CLT_TAG("Thread::Current::Exit final reschedule")};
  Scheduler::RescheduleInternal(current_thread);
  panic("somehow fell through thread_exit()\n");
}

void Thread::Current::Kill() {
  Thread* current_thread = Thread::Current::Get();

  current_thread->canary_.Assert();
  DEBUG_ASSERT(!current_thread->IsIdle());

  current_thread->Kill();
}

cpu_mask_t Thread::SetCpuAffinity(cpu_mask_t affinity) {
  canary_.Assert();
  return Scheduler::SetCpuAffinity(*this, affinity, AffinityType::Hard);
}

cpu_mask_t Thread::GetCpuAffinity() const {
  canary_.Assert();
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::GetCpuAffinity")};
  return scheduler_state_.hard_affinity();
}

cpu_mask_t Thread::SetSoftCpuAffinity(cpu_mask_t affinity) {
  canary_.Assert();
  return Scheduler::SetCpuAffinity(*this, affinity, AffinityType::Soft);
}

cpu_mask_t Thread::GetSoftCpuAffinity() const {
  canary_.Assert();
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::GetSoftCpuAffinity")};
  return scheduler_state_.soft_affinity_;
}

void Thread::Current::MigrateToCpu(const cpu_num_t target_cpu) {
  Thread::Current::Get()->SetCpuAffinity(cpu_num_to_mask(target_cpu));
}

void Thread::SetMigrateFn(MigrateFn migrate_fn) {
  Guard<SpinLock, IrqSave> list_guard{&get_list_lock()};
  SingleChainLockGuard thread_guard{NoIrqSaveOption, get_lock(), CLT_TAG("Thread::SetMigrateFn")};
  SetMigrateFnLocked(ktl::move(migrate_fn));
}

void Thread::SetMigrateFnLocked(MigrateFn migrate_fn) {
  Scheduler::RunInThreadsSchedulerLocked(this, [this, &migrate_fn]() {
    ChainLockTransaction::AssertActive();
    get_lock().AssertHeld();
    get_list_lock().lock().AssertHeld();

    // We should never be assigning a new migration function while there is a
    // migration still in progress.
    DEBUG_ASSERT(!migrate_fn || !migrate_pending_);

    // If |migrate_fn_| was previously set, remove |this| from |migrate_list_|.
    if (migrate_fn_) {
      migrate_list_.erase(*this);
    }

    migrate_fn_ = ktl::move(migrate_fn);

    // Clear stale state when (un) setting the migrate fn.
    //
    // TODO(https://fxbug.dev/42164826): Cleanup the migrate fn feature and associated state
    // and clearly define and check invariants.
    migrate_pending_ = false;

    // If |migrate_fn_| is valid, add |this| to |migrate_list_|.
    if (migrate_fn_) {
      migrate_list_.push_front(this);
    }
  });
}

void Thread::CallMigrateFnLocked(MigrateStage stage) {
  if (unlikely(migrate_fn_)) {
    switch (stage) {
      case MigrateStage::Save:
        if (!migrate_pending_) {
          // We are leaving our last CPU and calling our migration function as we
          // go.  Assert that we are running on the proper CPU, and clear our last
          // cpu bookkeeping to indicate that the migration has started.
          DEBUG_ASSERT_MSG(scheduler_state().last_cpu_ == arch_curr_cpu_num(),
                           "Attempting to run Before stage of migration on a CPU "
                           "which is not the last CPU the thread ran on (last cpu = "
                           "%u, curr cpu = %u)\n",
                           scheduler_state().last_cpu_, arch_curr_cpu_num());
          migrate_pending_ = true;
          migrate_fn_(this, stage);
        }
        break;

      case MigrateStage::Restore:
        if (migrate_pending_) {
          migrate_pending_ = false;
          migrate_fn_(this, stage);
        }
        break;

      case MigrateStage::Exiting:
        migrate_fn_(this, stage);
        break;
    }
  }
}

// Go over every thread which has a migrate function assigned to it, and which
// last ran on the current CPU, but is not in the ready state.  The current CPU
// is becoming unavailable, and the currently running thread is the last change
// anything is going to have to run on this CPU for a while.  If there are
// threads out there which are blocked, but which need to record some per-cpu
// state (eg; VCPU threads), this will be their last chance to do so.  The next
// time they become scheduled to run, they are going to run on a different CPU.
void Thread::CallMigrateFnForCpu(cpu_num_t cpu) {
  DEBUG_ASSERT(arch_ints_disabled());
  Guard<SpinLock, NoIrqSave> list_guard{&list_lock_};

  for (auto& thread : migrate_list_) {
    SingleChainLockGuard thread_guard{NoIrqSaveOption, thread.get_lock(),
                                      CLT_TAG("Thread::CallMigrateFnForCpu")};
    if (thread.state() != THREAD_READY && thread.scheduler_state().last_cpu_ == cpu) {
      thread.CallMigrateFnLocked(Thread::MigrateStage::Save);
    }
  }
}

void Thread::SetContextSwitchFn(ContextSwitchFn context_switch_fn) {
  canary_.Assert();
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::SetContextSwitchFn")};
  SetContextSwitchFnLocked(ktl::move(context_switch_fn));
}

void Thread::SetContextSwitchFnLocked(ContextSwitchFn context_switch_fn) {
  canary_.Assert();
  context_switch_fn_ = ktl::move(context_switch_fn);
}

bool Thread::CheckKillSignal() {
  if (signals() & THREAD_SIGNAL_KILL) {
    // Ensure we don't recurse into thread_exit.
    DEBUG_ASSERT(state() != THREAD_DEATH);
    return true;
  } else {
    return false;
  }
}

zx_status_t Thread::CheckKillOrSuspendSignal() const {
  const auto current_signals = signals();
  if (unlikely(current_signals & THREAD_SIGNAL_KILL)) {
    return ZX_ERR_INTERNAL_INTR_KILLED;
  }
  if (unlikely(current_signals & THREAD_SIGNAL_SUSPEND)) {
    return ZX_ERR_INTERNAL_INTR_RETRY;
  }
  return ZX_OK;
}

// finish suspending the current thread
void Thread::Current::DoSuspend() {
  Thread* const current_thread = Thread::Current::Get();

  // Note: After calling this callback, we must not return without
  // calling the callback with THREAD_USER_STATE_RESUME.  That is
  // because those callbacks act as barriers which control when it is
  // safe for the zx_thread_read_state()/zx_thread_write_state()
  // syscalls to access the userland register state kept by Thread.
  if (current_thread->user_thread_) {
    DEBUG_ASSERT(!arch_ints_disabled());
    current_thread->user_thread_->Suspending();
  }

  bool do_exit{false};
  {
    // Hold the thread's lock while we set up the suspend operation.
    SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("Thread::Current::DoSuspend")};

    // make sure we haven't been killed while the lock was dropped for the user callback
    if (current_thread->CheckKillSignal()) {
      // Exit the thread, but only after we have dropped our locks and
      // re-enabled IRQs.
      do_exit = true;
    }
    // Make sure the suspend signal wasn't cleared while we were running the
    // callback.
    else if (current_thread->signals() & THREAD_SIGNAL_SUSPEND) {
      Scheduler::RunInLockedCurrentScheduler([current_thread](Scheduler&) {
        ChainLockTransaction::AssertActive();
        current_thread->get_lock().AssertHeld();
        current_thread->set_suspended();
      });
      current_thread->clear_signals(THREAD_SIGNAL_SUSPEND);

      // directly invoke the context switch, since we've already manipulated
      // this thread's state. Note that our thread's lock will be dropped and
      // re-acquired as we block and later become re-scheduled.
      Scheduler::RescheduleInternal(current_thread);

      // If the thread was killed while we were suspended, we should not allow
      // it to resume.  We shouldn't call user_callback() with
      // THREAD_USER_STATE_RESUME in this case, because there might not have
      // been any request to resume the thread.
      if (current_thread->CheckKillSignal()) {
        // Exit the thread, but only after we have dropped our locks and
        // re-enabled IRQs.
        do_exit = true;
      }
    }
  }

  if (do_exit) {
    Thread::Current::Exit(0);
  }

  if (current_thread->user_thread_) {
    DEBUG_ASSERT(!arch_ints_disabled());
    current_thread->user_thread_->Resuming();
  }
}

void Thread::SignalSampleStack(Timer* timer, zx_instant_mono_t, void* session_id) {
  // Regardless of if the thread is marked to be sampled or not we'll set the sample_stack thread
  // signal. This reduces the time we spend in the interrupt context and means we don't need to grab
  // a lock here. When we handle the thread signal in ProcessPendingSignals we'll check if the
  // thread actually needs to be sampled.
  Thread* current_thread = Thread::Current::Get();
  current_thread->sampling_session_.store(reinterpret_cast<uint64_t>(session_id));
  current_thread->canary_.Assert();
  current_thread->set_signals(THREAD_SIGNAL_SAMPLE_STACK);

  // We set the timer here as opposed to when we handle the THREAD_SIGNAL_SAMPLE_STACK as a thread
  // could be suspended or killed before the sample signal is handled.
  sampler::gThreadSampler.RescheduleMarking();
}

void Thread::Current::DoSampleStack(GeneralRegsSource source, void* gregs) {
  DEBUG_ASSERT(!arch_ints_disabled());
  Thread* current_thread = Thread::Current::Get();

  // Make sure the sample signal wasn't cleared while we were running the
  // callback.
  if (current_thread->signals() & THREAD_SIGNAL_SAMPLE_STACK) {
    current_thread->clear_signals(THREAD_SIGNAL_SAMPLE_STACK);

    if (current_thread->user_thread() == nullptr) {
      // There's no user thread to sample, just move on.
      return;
    }

    // If a thread was marked to be sampled but was first suspended, it may now be long after the
    // sampling session has ended. sampler::SampleThread grabs the global state, checks if it's
    // still valid.
    auto sampler_result =
        SamplerDispatcher::SampleThread(current_thread->pid(), current_thread->tid(), source, gregs,
                                        current_thread->sampling_session_.load());
    if (sampler_result.is_error()) {
      kcounter_add(thread_sampling_failed, 1);
    }
  }
}

bool Thread::SaveUserStateLocked() {
  DEBUG_ASSERT(this == Thread::Current::Get());
  DEBUG_ASSERT(user_thread_ != nullptr);

  if (user_state_saved_) {
    return false;
  }
  user_state_saved_ = true;
  arch_save_user_state(this);
  return true;
}

void Thread::RestoreUserStateLocked() {
  DEBUG_ASSERT(this == Thread::Current::Get());
  DEBUG_ASSERT(user_thread_ != nullptr);

  DEBUG_ASSERT(user_state_saved_);
  user_state_saved_ = false;
  arch_restore_user_state(this);
}

ScopedThreadExceptionContext::ScopedThreadExceptionContext(const arch_exception_context_t* context)
    : thread_(Thread::Current::Get()), context_(context) {
  SingleChainLockGuard guard{IrqSaveOption, thread_->get_lock(),
                             CLT_TAG("ScopedThreadExceptionContext")};

  // It's possible that the context and state have been installed/saved earlier in the call chain.
  // If so, then it's some other object's responsibility to remove/restore.
  need_to_remove_ = arch_install_exception_context(thread_, context_);
  need_to_restore_ = thread_->SaveUserStateLocked();
}

ScopedThreadExceptionContext::~ScopedThreadExceptionContext() {
  SingleChainLockGuard guard{IrqSaveOption, thread_->get_lock(),
                             CLT_TAG("~ScopedThreadExceptionContext")};

  // Did we save the state?  If so, then it's our job to restore it.
  if (need_to_restore_) {
    thread_->RestoreUserStateLocked();
  }
  // Did we install the exception context? If so, then it's out job to remove it.
  if (need_to_remove_) {
    arch_remove_exception_context(thread_);
  }
}

void Thread::Current::ProcessPendingSignals(GeneralRegsSource source, void* gregs) {
  // Prior to calling this method, interrupts must be disabled.  This method may enable interrupts,
  // but if it does, it will disable them prior to returning.
  //
  // TODO(maniscalco): Refer the reader to the to-be-written theory of operations documentation for
  // kernel thread signals.
  DEBUG_ASSERT(arch_ints_disabled());

  Thread* const current_thread = Thread::Current::Get();

  // IF we're currently in restricted mode, we may decide to exit to normal mode instead as part
  // of signal processing. Store this information in a local so that we can process all signals
  // before actually exiting.
  bool exit_to_normal_mode = false;

  // It's possible for certain signals to be asserted, processed, and then re-asserted during this
  // method so loop until there are no pending signals.
  unsigned int signals;
  while ((signals = current_thread->signals()) != 0) {
    // THREAD_SIGNAL_KILL
    //
    // We check this signal first because if asserted, the thread will terminate so there's no point
    // in checking other signals before kill.
    if (signals & THREAD_SIGNAL_KILL) {
      // We're going to call Exit, which generates a user-visible exception on user threads.  If
      // this thread has user mode component, call arch_set_suspended_general_regs() to make the
      // general registers available to a debugger during the exception.
      if (current_thread->user_thread_) {
        // TODO(https://fxbug.dev/42076855): Do we need to hold the thread's lock here?
        SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                                   CLT_TAG("Thread::Current::ProcessPendingSignals")};
        arch_set_suspended_general_regs(current_thread, source, gregs);
      }

      // Thread::Current::Exit may only be called with interrupts enabled.  Note, this is thread is
      // committed to terminating and will never return so we don't need to worry about disabling
      // interrupts post-Exit.
      arch_enable_ints();
      Thread::Current::Exit(0);

      __UNREACHABLE;
    }

    const bool has_user_thread = current_thread->user_thread_ != nullptr;

    // THREAD_SIGNAL_POLICY_EXCEPTION
    //
    if (signals & THREAD_SIGNAL_POLICY_EXCEPTION) {
      DEBUG_ASSERT(has_user_thread);
      // TODO(https://fxbug.dev/42077109): Consider wrapping this up in a method
      // (e.g. Thread::Current::ClearSignals) and think hard about whether relaxed is sufficient.
      current_thread->clear_signals(THREAD_SIGNAL_POLICY_EXCEPTION);

      uint32_t policy_exception_code;
      uint32_t policy_exception_data;
      {
        // TODO(https://fxbug.dev/42076855): Do we need to hold the thread's lock here?
        SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                                   CLT_TAG("Thread::Current::ProcessPendingSignals")};

        // Policy exceptions are user-visible so must make the general register state available to
        // a debugger.
        arch_set_suspended_general_regs(current_thread, source, gregs);

        policy_exception_code = current_thread->extra_policy_exception_code_;
        policy_exception_data = current_thread->extra_policy_exception_data_;
      }

      // Call arch_dispatch_user_policy_exception with interrupts enabled, but be sure to disable
      // them afterwards.
      arch_enable_ints();
      zx_status_t status =
          arch_dispatch_user_policy_exception(policy_exception_code, policy_exception_data);
      ZX_ASSERT_MSG(status == ZX_OK, "arch_dispatch_user_policy_exception() failed: status=%d\n",
                    status);
      arch_disable_ints();

      {
        // TODO(https://fxbug.dev/42076855): Do we need to hold the thread's lock here?
        SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                                   CLT_TAG("Thread::Current::ProcessPendingSignals")};
        arch_reset_suspended_general_regs(current_thread);
      }
    }

    // THREAD_SIGNAL_POLICY_KILL
    //
    if (signals & THREAD_SIGNAL_POLICY_KILL) {
      DEBUG_ASSERT(has_user_thread);
      // TODO(https://fxbug.dev/42077109): Consider wrapping this up in a method
      // (e.g. Thread::Current::ClearSignals) and think hard about whether relaxed is sufficient.
      current_thread->clear_signals(THREAD_SIGNAL_POLICY_KILL);

      arch_enable_ints();
      // Kill will begin the sequence, but won't actually terminate the calling thread.  Instead, it
      // will raise THREAD_SIGNAL_KILL.  See that the signal was raised.  A subsequent iteration of
      // this loop will process THREAD_SIGNAL_KILL.
      ProcessDispatcher::GetCurrent()->Kill(ZX_TASK_RETCODE_POLICY_KILL);
      DEBUG_ASSERT((current_thread->signals() & THREAD_SIGNAL_KILL) != 0);
      arch_disable_ints();
    }

    // THREAD_SIGNAL_SUSPEND
    //
    if (signals & THREAD_SIGNAL_SUSPEND) {
      if (has_user_thread) {
        bool saved;
        {
          // TODO(https://fxbug.dev/42076855): Do we need to hold the thread's lock here?
          SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                                     CLT_TAG("Thread::Current::ProcessPendingSignals")};

          // This thread has been asked to suspend.  When a user thread is suspended, its full
          // register state (not just the general purpose registers) is accessible to a debugger.
          arch_set_suspended_general_regs(current_thread, source, gregs);
          saved = current_thread->SaveUserStateLocked();
        }

        // The enclosing function, is called at the boundary of kernel and user mode (e.g. just
        // before returning from a syscall, timer interrupt, or architectural exception/fault).
        // We're about the perform a save.  If the save fails (returns false), then we likely have a
        // mismatched save/restore pair, which is a bug.
        DEBUG_ASSERT(saved);

        arch_enable_ints();
        Thread::Current::DoSuspend();
        arch_disable_ints();

        {
          // TODO(https://fxbug.dev/42076855): Do we need to hold the thread's lock here?
          SingleChainLockGuard guard{NoIrqSaveOption, current_thread->get_lock(),
                                     CLT_TAG("Thread::Current::ProcessPendingSignals")};

          if (saved) {
            current_thread->RestoreUserStateLocked();
          }
          arch_reset_suspended_general_regs(current_thread);
        }
      } else {
        // No user mode component so we don't need to save any register state.
        arch_enable_ints();
        Thread::Current::DoSuspend();
        arch_disable_ints();
      }
    }

    // THREAD_SIGNAL_RESTRICTED_KICK
    //
    if (signals & THREAD_SIGNAL_RESTRICTED_KICK) {
      current_thread->clear_signals(THREAD_SIGNAL_RESTRICTED_KICK);
      if (arch_get_restricted_flag()) {
        exit_to_normal_mode = true;
      } else {
        // If we aren't currently in restricted mode,Remember that we have a restricted kick pending
        // for the next time we try to enter restricted mode.
        current_thread->set_restricted_kick_pending(true);
      }
    }

    // THREAD_SIGNAL_CHECK_RSEQ
    //
    if (signals & THREAD_SIGNAL_CHECK_RSEQ) {
      DEBUG_ASSERT(has_user_thread);
      current_thread->clear_signals(THREAD_SIGNAL_CHECK_RSEQ);
      current_thread->CheckRestartableSequence(source, gregs);
    }

    // THREAD_SIGNAL_SAMPLE_STACK
    //
    if (signals & THREAD_SIGNAL_SAMPLE_STACK) {
      // Sampling the user stack may page fault as we try to do usercopies.
      arch_enable_ints();
      Thread::Current::DoSampleStack(source, gregs);
      arch_disable_ints();
    }
  }

  // Interrupts must remain disabled for the remainder of this function. If for any reason we need
  // to enable interrupts in handling we have to re-enter the loop above in order to process signals
  // that may have been raised.

  if (exit_to_normal_mode) {
    switch (source) {
      case GeneralRegsSource::Iframe: {
        const iframe_t* iframe = reinterpret_cast<const iframe_t*>(gregs);
        RestrictedLeaveIframe(iframe, ZX_RESTRICTED_REASON_KICK);

        __UNREACHABLE;
      }
#if defined(__x86_64__)
      case GeneralRegsSource::Syscall: {
        const syscall_regs_t* syscall_regs = reinterpret_cast<const syscall_regs_t*>(gregs);
        RestrictedLeaveSyscall(syscall_regs, ZX_RESTRICTED_REASON_KICK);

        __UNREACHABLE;
      }
#endif  // defined(__x86_64__)
      default:
        DEBUG_ASSERT_MSG(false, "invalid source %u\n", static_cast<uint32_t>(source));
    }
  }
}

bool Thread::Current::CheckForRestrictedKick() {
  LTRACE_ENTRY;

  DEBUG_ASSERT(arch_ints_disabled());

  Thread* current_thread = Thread::Current::Get();
  if (current_thread->restricted_kick_pending()) {
    current_thread->set_restricted_kick_pending(false);
    return true;
  }

  return false;
}

/**
 * @brief Yield the cpu to another thread
 *
 * This function places the current thread at the end of the run queue
 * and yields the cpu to another waiting thread (if any.)
 *
 * This function will return at some later time. Possibly immediately if
 * no other threads are waiting to execute.
 */
void Thread::Current::Yield() {
  Thread* current_thread = Thread::Current::Get();

  current_thread->canary_.Assert();
  DEBUG_ASSERT(!arch_blocking_disallowed());

  SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                             CLT_TAG("Thread::Current::Yield")};
  DEBUG_ASSERT(current_thread->state() == THREAD_RUNNING);

  CPU_STATS_INC(yields);
  Scheduler::Yield(current_thread);
}

/**
 * @brief Preempt the current thread from an interrupt
 *
 * This function places the current thread at the head of the run
 * queue and then yields the cpu to another thread.
 */
void Thread::Current::Preempt() {
  Thread* current_thread = Thread::Current::Get();

  current_thread->canary_.Assert();
  DEBUG_ASSERT(!arch_blocking_disallowed());

  if (!current_thread->IsIdle()) {
    // only track when a meaningful preempt happens
    CPU_STATS_INC(preempts);
  }

  Scheduler::Preempt(Scheduler::PreemptType::Irq);
}

/**
 * @brief Reevaluate the run queue on the current cpu.
 *
 * This function places the current thread at the head of the run
 * queue and then yields the cpu to another thread. Similar to
 * thread_preempt, but intended to be used at non interrupt context.
 */
void Thread::Current::Reschedule() {
  Thread* current_thread = Thread::Current::Get();

  current_thread->canary_.Assert();
  DEBUG_ASSERT(!arch_blocking_disallowed());

  SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                             CLT_TAG("Thread::Current::Reschedule")};

  DEBUG_ASSERT(current_thread->state() == THREAD_RUNNING);
  Scheduler::Reschedule(current_thread);
}

void PreemptionState::SetPreemptionTimerForExtension(zx_instant_mono_t deadline) {
  // Interrupts must be disabled when calling PreemptReset.
  InterruptDisableGuard interrupt_disable;
  percpu::Get(arch_curr_cpu_num()).timer_queue.PreemptReset(deadline);
  kcounter_add(thread_timeslice_extended, 1);
}

void PreemptionState::FlushPendingContinued(Flush flush) {
  // If we're flushing the local CPU, make sure OK to block since flushing
  // local may trigger a reschedule.
  DEBUG_ASSERT(((flush & FlushLocal) == 0) || !arch_blocking_disallowed());

  InterruptDisableGuard interrupt_disable;
  // Recheck, pending preemptions could have been flushed by a context switch
  // before interrupts were disabled.
  const cpu_mask_t pending_mask = preempts_pending_;

  // If there is a pending local preemption the scheduler will take care of
  // flushing all pending reschedules.
  const cpu_mask_t current_cpu_mask = cpu_num_to_mask(arch_curr_cpu_num());
  if ((pending_mask & current_cpu_mask) != 0 && (flush & FlushLocal) != 0) {
    // Clear the local preempt pending flag before calling preempt.  Failure
    // to do this can cause recursion during Scheduler::Preempt if any code
    // (such as debug tracing code) attempts to disable and re-enable
    // preemption during the scheduling operation.
    preempts_pending_ &= ~current_cpu_mask;

    // TODO(johngro): We cannot be holding the current thread's lock while we do
    // this.  Is there a good way to enforce this requirement via either static
    // annotation or dynamic checks?
    Scheduler::Preempt(Scheduler::PreemptType::FlushPending);
  } else if ((flush & FlushRemote) != 0) {
    // The current cpu is ignored by mp_reschedule if present in the mask.
    mp_reschedule(pending_mask, 0);
    preempts_pending_ &= current_cpu_mask;
  }
}

// timer callback to wake up a sleeping thread
void Thread::SleepHandler(Timer* timer, zx_instant_mono_t now, void* arg) {
  Thread* t = static_cast<Thread*>(arg);
  t->canary_.Assert();
  t->HandleSleep(timer, now);
}

void Thread::HandleSleep(Timer* timer, zx_instant_mono_t now) {
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
    if (timer->TrylockOrCancel(get_lock())) {
      return ChainLockTransaction::Done;
    }

    // We only need to be holding the thread's lock in order "unsleep" it.  It is
    // not waiting in a wait queue, so there is no WaitQueue lock which needs to
    // be held.
    ChainLockTransaction::Finalize();

    if (state() != THREAD_SLEEPING) {
      get_lock().Release();
      return ChainLockTransaction::Done;
    }

    // Unblock the thread, regardless of whether the sleep was interruptible.  Not
    // that calling Unsleep will cause the scheduler to drop the thread's lock for
    // us after it has been successfully assigned to a scheduler instance.
    wait_queue_state_.Unsleep(this, ZX_OK);
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(IrqSaveOption, CLT_TAG("HandleSleep"), do_transaction);
}

#define MIN_SLEEP_SLACK ZX_USEC(1)
#define MAX_SLEEP_SLACK ZX_SEC(1)
#define DIV_SLEEP_SLACK 10u

// computes the amount of slack the thread_sleep timer will use
static zx_duration_mono_t sleep_slack(zx_instant_mono_t deadline, zx_instant_mono_t now) {
  if (deadline < now) {
    return MIN_SLEEP_SLACK;
  }
  zx_duration_mono_t slack = zx_time_sub_time(deadline, now) / DIV_SLEEP_SLACK;
  return ktl::max(MIN_SLEEP_SLACK, ktl::min(slack, MAX_SLEEP_SLACK));
}

/**
 * @brief  Put thread to sleep; deadline specified in ns
 *
 * This function puts the current thread to sleep until the specified
 * deadline has occurred.
 *
 * Note that this function could continue to sleep after the specified deadline
 * if other threads are running.  When the deadline occurrs, this thread will
 * be placed at the head of the run queue.
 *
 * interruptible argument allows this routine to return early if the thread was signaled
 * for something.
 */
zx_status_t Thread::Current::SleepEtc(const Deadline& deadline, Interruptible interruptible,
                                      zx_instant_mono_t now) {
  Thread* current_thread = Thread::Current::Get();

  current_thread->canary_.Assert();
  DEBUG_ASSERT(!current_thread->IsIdle());
  DEBUG_ASSERT(!arch_blocking_disallowed());

  // Skip all of the work if the deadline has already passed.
  if (deadline.when() <= now) {
    return ZX_OK;
  }

  Timer timer;
  zx_status_t final_blocked_status;
  {
    SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("Thread::Current::SleepEtc")};
    DEBUG_ASSERT(current_thread->state() == THREAD_RUNNING);

    // if we've been killed and going in interruptible, abort here
    if (interruptible == Interruptible::Yes && unlikely((current_thread->signals()))) {
      if (current_thread->signals() & THREAD_SIGNAL_KILL) {
        return ZX_ERR_INTERNAL_INTR_KILLED;
      } else {
        return ZX_ERR_INTERNAL_INTR_RETRY;
      }
    }

    // set a one shot timer to wake us up and reschedule
    timer.Set(deadline, &Thread::SleepHandler, current_thread);

    current_thread->set_sleeping();
    current_thread->wait_queue_state_.Block(current_thread, interruptible, ZX_OK);

    // Once we wake up again, we will be holding the current thread's lock once
    // again.  That said, the lock had been dropped, and was re-acquired while we
    // were blocked.  Stash our BlockedStatus while we are still holding our lock,
    // but make sure to drop the lock before we call into timer.Cancel().
    //
    // When we call into timer.Cancel(), if we discover a timer callback in
    // flight, we are going to spin until the timer callback flags itself as
    // completed, ensuring that it is safe for the Timer on our stack to be
    // destroyed.  The timer callback is going to need this lock as well, if it
    // gets run, however.  So, if we are still holding our lock when we call
    // cancel, we risk deadlock.
    //
    // always cancel the timer, since we may be racing with the timer tick on
    // other cpus
    final_blocked_status = current_thread->wait_queue_state_.BlockedStatus();
  }
  timer.Cancel();
  return final_blocked_status;
}

zx_status_t Thread::Current::Sleep(zx_instant_mono_t deadline) {
  const zx_instant_mono_t now = current_mono_time();
  return SleepEtc(Deadline::no_slack(deadline), Interruptible::No, now);
}

zx_status_t Thread::Current::SleepRelative(zx_duration_mono_t delay) {
  const zx_instant_mono_t now = current_mono_time();
  const Deadline deadline = Deadline::no_slack(zx_time_add_duration(now, delay));
  return SleepEtc(deadline, Interruptible::No, now);
}

zx_status_t Thread::Current::SleepInterruptible(zx_instant_mono_t deadline) {
  const zx_instant_mono_t now = current_mono_time();
  const TimerSlack slack(sleep_slack(deadline, now), TIMER_SLACK_LATE);
  const Deadline slackDeadline(deadline, slack);
  return SleepEtc(slackDeadline, Interruptible::Yes, now);
}

/**
 * @brief Return the number of nanoseconds a thread has been running for.
 *
 * This takes the thread's lock to ensure there are no races while calculating
 * the runtime of the thread.
 */
zx_duration_mono_t Thread::Runtime() const {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::Runtime")};

  zx_duration_mono_t runtime = scheduler_state_.runtime_ns();
  if (state() == THREAD_RUNNING) {
    zx_duration_mono_t recent =
        zx_time_sub_time(current_mono_time(), scheduler_state_.last_started_running());
    runtime = zx_duration_add_duration(runtime, recent);
  }

  return runtime;
}

/**
 * @brief Get the last CPU the given thread was run on, or INVALID_CPU if the
 * thread has never run.
 */
cpu_num_t Thread::LastCpu() const {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::LastCpu")};
  return scheduler_state_.last_cpu_;
}

/**
 * @brief Get the last CPU the given thread was run on, or INVALID_CPU if the
 * thread has never run.
 */
cpu_num_t Thread::LastCpuLocked() const { return scheduler_state_.last_cpu_; }

/**
 * @brief  Initialize threading system
 *
 * This function is called once, from kmain()
 */
void thread_init_early() {
  DEBUG_ASSERT(arch_curr_cpu_num() == 0);

  // Initialize the thread list. This needs to be done manually now, since initial thread code
  // manipulates the list before global constructors are run.
  []() TA_NO_THREAD_SAFETY_ANALYSIS { thread_list.Initialize(); }();

  // Init the boot percpu data.
  percpu::InitializeBoot();

  // create a thread to cover the current running state
  Thread* t = &percpu::Get(0).idle_power_thread.thread();
  ConstructFirstThread<ConstructFirstThreadBootCpu>(t, "bootstrap");
}

/**
 * @brief Change name of current thread
 */
void Thread::Current::SetName(const char* name) {
  Thread* current_thread = Thread::Current::Get();
  strlcpy(current_thread->name_, name, sizeof(current_thread->name_));
}

/**
 * @brief Change the base profile of current thread
 *
 * Changes the base profile of the thread to the base profile supplied by the
 * users, dealing with any side effects in the process.
 *
 * @param profile The base profile to apply to the thread.
 */
void Thread::SetBaseProfile(const SchedulerState::BaseProfile& profile) {
  canary_.Assert();
  OwnedWaitQueue::SetThreadBaseProfileAndPropagate(*this, profile);
}

/**
 * @brief Set the pointer to the user-mode thread, this will receive callbacks:
 * ThreadDispatcher::Exiting()
 * ThreadDispatcher::Suspending() / Resuming()
 *
 * This also caches the assocatiated koids of the thread and process
 * dispatchers associated with the given ThreadDispatcher.
 */
void Thread::SetUsermodeThread(fbl::RefPtr<ThreadDispatcher> user_thread) {
  canary_.Assert();

  SingleChainLockGuard thread_guard{IrqSaveOption, get_lock(),
                                    CLT_TAG("Thread::SetUsermodeThread")};
  DEBUG_ASSERT(state() == THREAD_INITIAL);
  DEBUG_ASSERT(!user_thread_);

  user_thread_ = ktl::move(user_thread);
  tid_ = user_thread_->get_koid();
  pid_ = user_thread_->process()->get_koid();

  // All user mode threads are detached since they are responsible for cleaning themselves up.
  // We can set this directly because we've checked that we are in the initial state.
  flags_ |= THREAD_FLAG_DETACHED;
}

/**
 * @brief  Become an idle thread
 *
 * This function marks the current thread as the idle thread -- the one which
 * executes when there is nothing else to do.  This function does not return.
 * This thread is called once at boot on the first cpu.
 */
void Thread::Current::BecomeIdle() {
  DEBUG_ASSERT(arch_ints_disabled());

  Thread* t = Thread::Current::Get();
  cpu_num_t curr_cpu = arch_curr_cpu_num();

  {
    // Hold our own lock while we fix up our bookkeeping
    SingleChainLockGuard thread_guard{NoIrqSaveOption, t->get_lock(),
                                      CLT_TAG("Thread::Current::BecomeIdle")};

    // Set our name
    char name[16];
    snprintf(name, sizeof(name), "idle %u", curr_cpu);
    Thread::Current::SetName(name);

    // Mark ourself as idle
    t->flags_ |= THREAD_FLAG_IDLE;

    // Now that we are the idle thread, make sure that we drop out of the
    // scheduler's bookkeeping altogether.
    Scheduler::RemoveFirstThread(t);
    Scheduler::UnblockIdle(t);
    t->set_running();

    // Cpu must be marked active by now, indicate that it is idle as well.
    DEBUG_ASSERT(Scheduler::PeekIsActive(curr_cpu));
    Scheduler::Get()->SetIdle(true);

    // Pend a preemption to ensure a reschedule.
    arch_set_blocking_disallowed(true);
    t->preemption_state().PreemptSetPending();
    arch_set_blocking_disallowed(false);
  }

  // Re-enable interrupts, re-enable preemption, drop into our idle routine, and
  // never return.
  arch_enable_ints();
  t->preemption_state().PreemptReenable();
  DEBUG_ASSERT(t->preemption_state().PreemptIsEnabled());

  IdlePowerThread::Run(nullptr);
  __UNREACHABLE;
}

/**
 * @brief Create a thread around the current execution context, preserving |t|'s stack
 *
 * Prior to calling, |t->stack| must be properly constructed. See |vm_allocate_kstack|.
 */
void Thread::SecondaryCpuInitEarly() {
  DEBUG_ASSERT(arch_ints_disabled());
  DEBUG_ASSERT(stack_.base() != 0);
  DEBUG_ASSERT(IS_ROUNDED(this, alignof(Thread)));

  // At this point, the CPU isn't far enough along to allow threads to block. Set blocking
  // disallowed until to catch bugs where code might block before we're ready.
  arch_set_blocking_disallowed(true);

  percpu::InitializeSecondaryFinish();

  char name[16];
  snprintf(name, sizeof(name), "cpu_init %u", arch_curr_cpu_num());
  ConstructFirstThread<ConstructFirstThreadSecondaryCpu>(this, name);

  // Emitting the thread metadata usually happens during Thread::Resume(), however, cpu_init threads
  // are never resumed. Emit the metadata here so that the thread name is associated with its tid.
  KTRACE_KERNEL_OBJECT("kernel:meta", this->tid(), ZX_OBJ_TYPE_THREAD, this->name(),
                       ("process", ktrace::Koid(this->pid())), ("job", ktrace::Koid(0)));
}

/**
 * @brief The last routine called on the secondary cpu's bootstrap thread.
 */
void thread_secondary_cpu_entry() {
  DEBUG_ASSERT(arch_blocking_disallowed());

  Scheduler::SetCurrCpuActive(true);

  percpu& current_cpu = percpu::GetCurrent();

  // Signal the idle/power thread to transition to active but don't wait for it, since it cannot run
  // until this thread either blocks or exits below. The idle thread will run immediately upon exit
  // and complete the transition, if necessary.
  const IdlePowerThread::TransitionResult result =
      current_cpu.idle_power_thread.TransitionOfflineToActive(ZX_TIME_INFINITE_PAST);

  // The first time a secondary CPU becomes active after boot the CPU power thread is already in the
  // active state. If the CPU power thread is not in its initial active state, it is being returned
  // to active from offline and needs to be revived to resume its normal function.
  if (result.starting_state != IdlePowerThread::State::Active) {
    Thread::ReviveIdlePowerThread(arch_curr_cpu_num());
  }

  // CAREFUL: This must happen after the idle/power thread is revived, since creating the DPC thread
  // can contend on VM locks and could cause this CPU to go idle.
  current_cpu.dpc_runner.InitForCurrentCpu();

  // Remove ourselves from the Scheduler's bookkeeping.
  {
    Thread& current = *Thread::Current::Get();
    SingleChainLockGuard guard{IrqSaveOption, current.get_lock(),
                               CLT_TAG("thread_secondary_cpu_entry")};
    Scheduler::RemoveFirstThread(&current);
  }

  mp_signal_curr_cpu_ready();

  // Exit from our bootstrap thread, and enter the scheduler on this cpu.
  Thread::Current::Exit(0);
}

/**
 * @brief Create an idle thread for a secondary CPU
 */
Thread* Thread::CreateIdleThread(cpu_num_t cpu_num) {
  DEBUG_ASSERT(cpu_num != 0 && cpu_num < SMP_MAX_CPUS);

  char name[16];
  snprintf(name, sizeof(name), "idle %u", cpu_num);

  Thread* t = Thread::CreateEtc(&percpu::Get(cpu_num).idle_power_thread.thread(), name,
                                IdlePowerThread::Run, nullptr,
                                SchedulerState::BaseProfile{IDLE_PRIORITY}, nullptr);
  if (t == nullptr) {
    return t;
  }

  {
    SingleChainLockGuard guard{IrqSaveOption, t->get_lock(), CLT_TAG("Thread::CreateIdleThread")};

    t->flags_ |= THREAD_FLAG_IDLE | THREAD_FLAG_DETACHED;
    t->scheduler_state_.hard_affinity_ = cpu_num_to_mask(cpu_num);

    Scheduler::UnblockIdle(t);
  }
  return t;
}

void Thread::ReviveIdlePowerThread(cpu_num_t cpu_num) {
  DEBUG_ASSERT(cpu_num != 0 && cpu_num < SMP_MAX_CPUS);
  Thread* thread = &percpu::Get(cpu_num).idle_power_thread.thread();

  SingleChainLockGuard guard{IrqSaveOption, thread->get_lock(),
                             CLT_TAG("Thread::ReviveIdlePowerThread")};

  DEBUG_ASSERT(thread->flags() & THREAD_FLAG_IDLE);
  DEBUG_ASSERT(thread->scheduler_state().hard_affinity() == cpu_num_to_mask(cpu_num));
  DEBUG_ASSERT(thread->task_state().entry() == IdlePowerThread::Run);

  arch_thread_initialize(thread, reinterpret_cast<vaddr_t>(Thread::Trampoline));
  thread->preemption_state().Reset();
}

void Thread::CheckRestartableSequence(GeneralRegsSource source, void* gregs) {
  DEBUG_ASSERT(arch_ints_disabled());

  // Check if the restartable sequence has been removed since the signal was generated.
  //
  // This situation can occur if the thread was processing a `zx_thread_set_rseq` syscall that was
  // going to clear its accessor when it was preempted. When the scheduler resumes the thread, the
  // `rseq_accessor_` will be valid, and the scheduler will raise the signal to check the
  // restartable sequence. However, the `zx_thread_set_rseq` syscall will resume and clear the
  // accessor before the signal handler is called, so we need to check if the accessor is still
  // valid.
  if (!rseq_accessor_.IsValid()) {
    return;
  }

  // Update the CPU ID in the restartable sequence structure.
  const cpu_num_t cpu_id = arch_curr_cpu_num();
  rseq_accessor_.Write<&zx_rseq_t::cpu_id>(cpu_id);

  const zx_vaddr_t ip = arch_get_instruction_pointer(source, gregs);

  zx_vaddr_t start_ip;
  rseq_accessor_.Read<&zx_rseq_t::start_ip>(start_ip);

  zx_vaddr_t post_commit_offset;
  rseq_accessor_.Read<&zx_rseq_t::post_commit_offset>(post_commit_offset);

  if (ip - start_ip >= post_commit_offset) {
    // The restartable sequence has completed.
    return;
  }

  // The restartable sequence was interrupted. Restart it.
  zx_vaddr_t abort_ip;
  rseq_accessor_.Read<&zx_rseq_t::abort_ip>(abort_ip);

  zx_status_t status = arch_set_return_instruction_pointer(source, gregs, abort_ip);
  if (status != ZX_OK) {
    // TODO(https://fxbug.dev/449980934): Decide what we should do if we cannot set the
    // instruction pointer. Perhaps raise a policy exception?
    kcounter_add(rseq_bad_ip, 1);
  }
}

/**
 * @brief Return the name of the "owner" of the thread.
 *
 * Returns "kernel" if there is no owner.
 */

void Thread::OwnerName(char (&out_name)[ZX_MAX_NAME_LEN]) const {
  if (user_thread_) {
    [[maybe_unused]] zx_status_t status = user_thread_->process()->get_name(out_name);
    DEBUG_ASSERT(status == ZX_OK);
    return;
  }
  memcpy(out_name, "kernel", 7);
}

static const char* thread_state_to_str(enum thread_state state) {
  switch (state) {
    case THREAD_INITIAL:
      return "init";
    case THREAD_SUSPENDED:
      return "susp";
    case THREAD_READY:
      return "rdy";
    case THREAD_RUNNING:
      return "run";
    case THREAD_BLOCKED:
    case THREAD_BLOCKED_READ_LOCK:
      return "blok";
    case THREAD_SLEEPING:
      return "slep";
    case THREAD_DEATH:
      return "deth";
    default:
      return "unkn";
  }
}

/**
 * @brief  Dump debugging info about the specified thread.
 */
void ThreadDumper::DumpLocked(const Thread* t, const char* match_owner, bool full_dump) {
  if (!t->canary().Valid()) {
    dprintf(INFO, "dump_thread WARNING: thread at %p has bad magic\n", t);
  }

  char oname[ZX_MAX_NAME_LEN];
  t->OwnerName(oname);
  if (match_owner && strcmp(oname, match_owner) != 0) {
    return;
  }

  zx_duration_mono_t runtime = t->scheduler_state().runtime_ns();
  if (t->state() == THREAD_RUNNING) {
    zx_duration_mono_t recent =
        zx_time_sub_time(current_mono_time(), t->scheduler_state().last_started_running());
    runtime = zx_duration_add_duration(runtime, recent);
  }

  char profile_str[64]{0};
  if (const SchedulerState::EffectiveProfile& ep = t->scheduler_state().effective_profile();
      ep.IsFair()) {
    snprintf(profile_str, sizeof(profile_str), "Fair (w %ld)", ep.weight().raw_value());
  } else {
    DEBUG_ASSERT(ep.IsDeadline());
    snprintf(profile_str, sizeof(profile_str), "Deadline (c,d = %ld,%ld)",
             ep.deadline().capacity_ns.raw_value(), ep.deadline().deadline_ns.raw_value());
  }

  if (full_dump) {
    dprintf(INFO, "dump_thread: t %p (%s:%s)\n", t, oname, t->name());
    dprintf(INFO,
            "\tstate %s, curr/last cpu %d/%d, hard_affinity %#x, soft_cpu_affinity %#x, "
            "%s, remaining time slice %" PRIi64 "\n",
            thread_state_to_str(t->state()), (int)t->scheduler_state().curr_cpu(),
            (int)t->scheduler_state().last_cpu(), t->scheduler_state().hard_affinity(),
            t->scheduler_state().soft_affinity(), profile_str,
            t->scheduler_state().remaining_time_slice_ns().raw_value());
    dprintf(INFO, "\truntime_ns %" PRIi64 ", runtime_s %" PRIi64 "\n", runtime,
            runtime / 1000000000);
    t->stack().DumpInfo(INFO);
    dprintf(INFO, "\tentry %p, arg %p, flags 0x%x %s%s%s%s\n", t->task_state_.entry_,
            t->task_state_.arg_, t->flags_, (t->flags_ & THREAD_FLAG_DETACHED) ? "Dt" : "",
            (t->flags_ & THREAD_FLAG_FREE_STRUCT) ? "Ft" : "",
            (t->flags_ & THREAD_FLAG_IDLE) ? "Id" : "", (t->flags_ & THREAD_FLAG_VCPU) ? "Vc" : "");

    dprintf(INFO, "\twait queue %p, blocked_status %d, interruptible %s, wait queues owned %s\n",
            t->wait_queue_state().blocking_wait_queue_, t->wait_queue_state().blocked_status_,
            t->wait_queue_state().interruptible_ == Interruptible::Yes ? "yes" : "no",
            t->owned_wait_queues_.is_empty() ? "no" : "yes");

    dprintf(INFO, "\taspace %p\n", t->GetAspaceRefLocked().get());
    dprintf(INFO, "\tuser_thread %p, pid %" PRIu64 ", tid %" PRIu64 "\n", t->user_thread_.get(),
            t->pid(), t->tid());
    arch_dump_thread(t);
  } else {
    printf("thr %p st %4s owq %d %s pid %" PRIu64 " tid %" PRIu64 " run_ns %" PRIi64 " (%s:%s)\n",
           t, thread_state_to_str(t->state()), !t->owned_wait_queues_.is_empty(), profile_str,
           t->pid(), t->tid(), runtime, oname, t->name());
  }
}

void Thread::Dump(bool full) const {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::Dump")};
  ThreadDumper::DumpLocked(this, nullptr, full);
}

/**
 * @brief  Dump debugging info about all threads
 */
void Thread::DumpAllLocked(const char* match_owner, bool full) {
  for (const Thread& t : thread_list.Get()) {
    if (!t.canary().Valid()) {
      dprintf(INFO, "bad magic on thread struct %p, aborting.\n", &t);
      hexdump(&t, sizeof(Thread));
      break;
    }

    {
      SingleChainLockGuard guard{IrqSaveOption, t.get_lock(), CLT_TAG("Thread::DumpAllLocked")};
      ThreadDumper::DumpLocked(&t, match_owner, full);
    }
  }
}

void Thread::DumpAll(const char* match_owner, bool full) {
  Guard<SpinLock, IrqSave> list_guard{&list_lock_};
  DumpAllLocked(match_owner, full);
}

void Thread::DumpTid(zx_koid_t tid, bool full) {
  Guard<SpinLock, IrqSave> list_guard{&list_lock_};
  DumpTidLocked(tid, full);
}

void Thread::DumpTidLocked(zx_koid_t tid, bool full) {
  for (const Thread& t : thread_list.Get()) {
    if (t.tid() != tid) {
      continue;
    }

    if (!t.canary().Valid()) {
      dprintf(INFO, "bad magic on thread struct %p, aborting.\n", &t);
      hexdump(&t, sizeof(Thread));
      break;
    }

    // TODO(johngro): Is it ok to stop searching now?  In theory, no two threads
    // should share a TID.
    t.Dump(full);
    break;
  }
}

Thread* thread_id_to_thread_slow(zx_koid_t tid) {
  for (Thread& t : thread_list.Get()) {
    if (t.tid() == tid) {
      return &t;
    }
  }

  return nullptr;
}

/** @} */

// Used by ktrace at the start of a trace to ensure that all
// the running threads, processes, and their names are known
void ktrace_report_live_threads() {
  Guard<SpinLock, IrqSave> guard{&Thread::get_list_lock()};
  for (Thread& t : thread_list.Get()) {
    t.canary().Assert();
    const zx_koid_t job_koid = t.user_thread() ? t.user_thread()->process()->job()->get_koid() : 0u;
    KTRACE_KERNEL_OBJECT_ALWAYS(t.tid(), ZX_OBJ_TYPE_THREAD, t.name(),
                                ("process", ktrace::Koid(t.pid())),
                                ("job", ktrace::Koid(job_koid)));
  }
}

void Thread::UpdateRuntimeStats(thread_state new_state) {
  if (user_thread_) {
    user_thread_->UpdateRuntimeStats(new_state);
  }
}

fbl::RefPtr<VmAspace> Thread::GetAspaceRef() const {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::GetAspaceRef")};
  return GetAspaceRefLocked();
}

fbl::RefPtr<VmAspace> Thread::GetAspaceRefLocked() const {
  VmAspace* const cur_aspace = aspace_;
  if (!cur_aspace || scheduler_state().state() == THREAD_DEATH) {
    return nullptr;
  }

  cur_aspace->AddRef();
  return fbl::ImportFromRawPtr(cur_aspace);
}

namespace {

// TODO(maniscalco): Consider moving this method to the KernelStack class.
// That's probably a better home for it.
zx_status_t ReadStack(Thread* thread, vaddr_t ptr, vaddr_t* out, size_t sz) {
  if (!is_kernel_address(ptr) || (ptr < thread->stack().base()) ||
      (ptr > (thread->stack().top() - sz))) {
    return ZX_ERR_NOT_FOUND;
  }
  memcpy(out, reinterpret_cast<const void*>(ptr), sz);
  return ZX_OK;
}

void GetBacktraceCommon(Thread* thread, vaddr_t fp, Backtrace& out_bt) {
  // Be sure that all paths out of this function leave with |out_bt| either
  // properly filled in or empty.
  out_bt.reset();

  // Without frame pointers, dont even try.  The compiler should optimize out
  // the body of all the callers if it's not present.
  if (!WITH_FRAME_POINTERS) {
    return;
  }

  // Perhaps we don't yet have a thread context?
  if (thread == nullptr) {
    return;
  }

  if (fp == 0) {
    return;
  }

  vaddr_t pc;
  size_t n = 0;
  for (; n < Backtrace::kMaxSize; n++) {
    vaddr_t actual_fp = fp;

    // RISC-V has a nonstandard frame pointer which points to the CFA instead of
    // the previous frame pointer. Since the frame pointer and return address are
    // always just below the CFA, subtract 16 bytes to get to the actual frame pointer.
#if __riscv
    actual_fp -= 16;
#endif

    if (ReadStack(thread, actual_fp + 8, &pc, sizeof(vaddr_t))) {
      break;
    }
    out_bt.push_back(pc);
    if (ReadStack(thread, actual_fp, &fp, sizeof(vaddr_t))) {
      break;
    }
  }
}

}  // namespace

void Thread::Current::GetBacktrace(Backtrace& out_bt) {
  auto fp = reinterpret_cast<vaddr_t>(__GET_FRAME(0));
  GetBacktraceCommon(Thread::Current::Get(), fp, out_bt);

  // (https://fxbug.dev/42179766): Force the function to not tail call GetBacktraceCommon.
  // This will make sure the frame pointer we grabbed at the top
  // of the function is still valid across the call.
  __asm__ volatile("");
}

void Thread::Current::GetBacktrace(vaddr_t fp, Backtrace& out_bt) {
  GetBacktraceCommon(Thread::Current::Get(), fp, out_bt);
}

void Thread::GetBacktrace(Backtrace& out_bt) {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::GetBacktrace")};

  // Get the starting point if it's in a usable state.
  vaddr_t fp = 0;
  switch (state()) {
    case THREAD_BLOCKED:
    case THREAD_BLOCKED_READ_LOCK:
    case THREAD_SLEEPING:
    case THREAD_SUSPENDED:
      // Thread is blocked, so ask the arch code to get us a starting point.
      fp = arch_thread_get_blocked_fp(this);
      break;
    default:
      // Not in a valid state, can't get a backtrace.  Reset it so the caller
      // doesn't inadvertently use a previous value.
      out_bt.reset();
      return;
  }

  GetBacktraceCommon(this, fp, out_bt);
}

SchedulerState::EffectiveProfile Thread::SnapshotEffectiveProfile() const {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(),
                             CLT_TAG("Thread::SnapshotEffectiveeProfile")};
  SchedulerState::EffectiveProfile ret = SnapshotEffectiveProfileLocked();
  return ret;
}

SchedulerState::BaseProfile Thread::SnapshotBaseProfile() const {
  SingleChainLockGuard guard{IrqSaveOption, get_lock(), CLT_TAG("Thread::SnapshotBaseProfile")};
  SchedulerState::BaseProfile ret = SnapshotBaseProfileLocked();
  return ret;
}

ScopedMemoryStall::ScopedMemoryStall() {
  Thread* const current_thread = Thread::Current::Get();

  AutoPreemptDisabler preempt_disable;

  if (current_thread->memory_stall_state() == ThreadStallState::IgnoredKernelOnly) {
    return;
  }

  DEBUG_ASSERT(current_thread->memory_stall_state() == ThreadStallState::Progressing);
  current_thread->set_memory_stall_state(ThreadStallState::Stalling);
  percpu::GetCurrent().memory_stall_accumulator.Update(-1, +1);
}

ScopedMemoryStall::~ScopedMemoryStall() {
  Thread* const current_thread = Thread::Current::Get();

  AutoPreemptDisabler preempt_disable;

  if (current_thread->memory_stall_state() == ThreadStallState::IgnoredKernelOnly) {
    return;
  }

  DEBUG_ASSERT(current_thread->memory_stall_state() == ThreadStallState::Stalling);
  current_thread->set_memory_stall_state(ThreadStallState::Progressing);
  percpu::GetCurrent().memory_stall_accumulator.Update(+1, -1);
}
