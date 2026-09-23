// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/affine/ratio.h>
#include <lib/wake-vector.h>
#include <zircon/syscalls/system.h>
#include <zircon/types.h>

#include <kernel/idle_power_thread.h>
#include <kernel/thread.h>

namespace wake_vector {

namespace {
template <typename CountType>
CountType LimitSizeT(size_t size) {
  return static_cast<CountType>(std::min<size_t>(size, ktl::numeric_limits<CountType>::max()));
}
}  // namespace

WakeEvent::GlobalList WakeEvent::global_list_;
WakeEvent::PendingList WakeEvent::pending_list_;

void WakeEvent::Initialize() {
  // Stash our WakeVector constants in our report_info.  These will never change over the lifetime
  // of the WakeEvent, so we might as well just stash it all now.
  WakeVector::Diagnostics diagnostic_info;
  wake_vector_.GetDiagnostics(diagnostic_info);

  Guard<Mutex> guard(GlobalListLock::Get());
  DEBUG_ASSERT(!in_global_list());

  {
    Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};
    DEBUG_ASSERT(!in_pending_list());

    report_info_ = zx_wake_source_report_entry_t{0};
    report_info_.koid = diagnostic_info.koid;

    static_assert(sizeof(report_info_.name) == diagnostic_info.extra.size());
    ::memcpy(report_info_.name, diagnostic_info.extra.data(), sizeof(report_info_.name));

    // Initially flag our report entry as having been reported.  This sets up
    // our bookkeeping properly so the first time it becomes signaled, it will
    // initialize all of the fields instead of simply bumping the signal count
    // and updating the last signal time.
    AssignHasBeenReported(true);
  }

  global_list_.push_back(this);
}

void WakeEvent::Destroy() {
  Guard<Mutex> guard(GlobalListLock::Get());
  DEBUG_ASSERT(in_global_list());

  {
    // If we are in pending list right now, we need to remove ourselves, making
    // certain to ack at the IdlePowerThread if needed.
    Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};

    if (in_pending_list()) {
      if (is_signaled()) {
        IdlePowerThread::AcknowledgeSystemWakeEvent();
        AssignSignaled(false);
      }
      pending_list_.erase(*this);
    } else {
      // It should not be possible to have a pending ack, but not be on the
      // pending list.
      DEBUG_ASSERT(!is_signaled());
    }
  }

  global_list_.erase(*this);
}

WakeResult WakeEvent::TriggerLocked(zx_instant_boot_t trigger_time) {
  // There are three cases to consider here:
  //
  // 1) We are already signaled.  This is an unusual state to be in as we are
  //    being triggered again, even though we have never received an ack.
  //    Regardless we should just bump our signal count, and record a new last
  //    triggered time.
  // 2) We are not signaled, but we have not been reported yet.  We must have
  //    have been acknowledged previously, and are becoming triggered once
  //    again.  Just like case #1, bump the signal count and record a new last
  //    triggered time.
  // 3) We are not signaled, and have been reported (note; this is also the
  //    initial state of a WakeEvent).  It is time to re-initialize our report
  //    entry state to record the first trigger event.
  if (!is_signaled() && has_been_reported()) {
    report_info_.initial_signal_time = trigger_time;
    report_info_.last_ack_time = ZX_TIME_INFINITE;
    report_info_.signal_count = 1;
    AssignHasBeenReported(false);
  } else {
    ++report_info_.signal_count;
  }

  // Regardless of whether this was the first trigger, or a subsequent trigger,
  // record a new last trigger time.
  report_info_.last_signal_time = trigger_time;

  // If we are not in the pending list, add ourself now.  Now, we add ourself to
  // the front of the list instead of the back in order to prevent this one wake
  // source from being double-reported in the case that it has been added to a
  // report, and becomes re-triggered before report generation has finished.
  // See the extensive comment near the end of the WakeEvent class in
  // "wake-vector.h" for more details.
  if (!in_pending_list()) {
    pending_list_.push_front(this);
  }

  // If we are triggered multiple times without being ack'ed, make sure to
  // report only one trigger to the IdlePowerThread level.
  if (!is_signaled()) {
    AssignSignaled(true);

    // WakeEvent triggering can happen starting from one of two places.
    //
    // 1) In a hard IRQ handler because a physical interrupt wake source was triggered.
    // 2) During a user's call to zx_interrupt_trigger for a virtual interrupt.
    //
    // When we call TriggerSystemWakeEvent, if it needs to wake the boot CPU, it
    // is going to eventually call PreemptSetPending for the boot CPU in order
    // to make sure that it gets scheduled instead of simply returning to the
    // idle thread.  This function is going to demand that the per-cpu "blocking
    // disallowed" flag has been set.
    //
    // In the case of #1, this will always be the case.  We are in an IRQ
    // handler, and the flag is going to be automatically set/cleared at the
    // start of the handler (you are not allowed to block during an IRQ
    // handler).
    //
    // In the case of #2, this is _effectively_ true, but the per-cpu flag will
    // not have been set.  By the time we make it to this point (in
    // TriggerLocked) we will have interrupts off, and will be holding the wake
    // event pending list spinlock, both of which are Very Good Reasons to
    // disallow blocking.  This said, disabling interrupts and obtaining a
    // spinlock does _not_ automatically set the blocking-disallowed flag.  Why?
    // Because when a thread actually blocks, it needs to hold (among other
    // things) its scheduler's spinlock as it re-schedules after joining its
    // WaitQueue.
    //
    // We already have interrupts off, are holding a spinlock, and are not going
    // to call Scheduler::Block here.  So go ahead and set the blocking
    // disallowed flag while we call TriggerSystemWakeEvent, restoring the state
    // to its previous value at the end of the sequence.
    //
    DEBUG_ASSERT(arch_ints_disabled());
    const bool was_blocking_disallowed = arch_blocking_disallowed();
    arch_set_blocking_disallowed(true);
    auto cleanup = fit::defer(
        [was_blocking_disallowed]() { arch_set_blocking_disallowed(was_blocking_disallowed); });

    return IdlePowerThread::TriggerSystemWakeEvent();
  }

  return WakeResult::BadState;
}

void WakeEvent::AcknowledgeLocked(zx_instant_boot_t trigger_time, AckBehavior ack_behavior) {
  // In order for an event to be waiting to be acknowledged, it must be in the
  // pending list, and it needs to be signaled (we should not be getting spurious
  // double acks)
  DEBUG_ASSERT(in_pending_list());
  DEBUG_ASSERT(is_signaled());

  // Always record the last ack time.
  report_info_.last_ack_time = trigger_time;

  if (ack_behavior == AckBehavior::ClearSignaled) {
    // If our AckBehavior tells us to clear our signaled, then ack at the
    // IdlePowerThread level, and clear the signaled flag.
    IdlePowerThread::AcknowledgeSystemWakeEvent();
    AssignSignaled(false);
  } else {
    // We have been told that we should remain signaled.  Clear our "has been
    // reported" flag.  We are in a situation where the interrupt was triggered
    // at least once more after the first time.  The first trigger may have been
    // reported, but the second and subsequent triggers have not been reported
    // yet.
    AssignHasBeenReported(false);
  }

  // Do NOT remove this event from the pending list, even if it has been
  // reported already.  If we remove the event from the list, we run the risk of
  // invalidating the iterator for a report which is currently being generated.
  // Instead, just leave the event it place, and let report generation handle
  // removing the event from the list when the time comes.
  //
  // See the extensive comment in `wake-vector.h` for a more comprehensive
  // explanation of the pending list and the various ways that operations
  // interact with it, and the things they need to do in order to remain safe.
}

void WakeEvent::Dump(FILE* f, zx_instant_boot_t log_triggered_after_boot_time) {
  Guard<Mutex> guard(GlobalListLock::Get());
  Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};

  for (auto iter = global_list_.cbegin(); iter.IsValid(); ++iter) {
    if (iter->is_signaled() ||
        iter->report_info_.last_signal_time >= log_triggered_after_boot_time) {
      const zx_wake_source_report_entry_t entry = iter->report_info_;
      pending_guard.CallUnlocked([f, &entry]() {
        // clang-format off
        fprintf(f, "  koid            : %" PRIu64 "\n"
                   "  pending         : %s\n"
                   "  prev reported   : %s\n"
                   "  signal count    : %u\n"
                   "  initial trigger : %" PRIi64 "\n"
                   "  last trigger    : %" PRIi64 "\n"
                   "  last ack        : %" PRIi64 "\n"
                   "  extra           : %.*s\n\n",
                entry.koid,
                entry.flags & ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED ? "yes" : "no",
                entry.flags & ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED ? "yes" : "no",
                entry.signal_count,
                entry.initial_signal_time,
                entry.last_signal_time,
                entry.last_ack_time,
                static_cast<int>(ktl::size(entry.name)), entry.name);
        // clang-format on
      });
    }
  }
}

zx_status_t WakeEvent::GenerateWakeEventReport(
    zx_instant_boot_t suspend_start_time, user_out_ptr<zx_wake_source_report_header_t> out_header,
    user_out_ptr<zx_wake_source_report_entry_t> out_entries, uint32_t num_entries,
    user_out_ptr<uint32_t> actual_entries) {
  // These should have been verified for us already at the syscall layer.
  DEBUG_ASSERT(static_cast<bool>(out_header));

  const bool do_report_entries = static_cast<bool>(out_entries);
  DEBUG_ASSERT(do_report_entries == (num_entries != 0));
  DEBUG_ASSERT(do_report_entries == static_cast<bool>(actual_entries));

  zx_wake_source_report_header_t hdr{0};
  uint32_t reported_entries{0};
  hdr.suspend_start_time = suspend_start_time;

  // We are going to need to heap allocate a side buffer to copy our results into.  We cannot copy
  // our results back out to user mode while holding any of our locks, and we cannot allocate an
  // arbitrary amount of entries on our stack.
  ktl::unique_ptr<zx_wake_source_report_entry_t[]> entry_side_buffer;
  size_t entry_side_buffer_len{0};

  {
    // Hold the global list lock for the entire O(n) time it takes to create the
    // report.  This prevents any wake sources from being created or destroyed
    // while we iterate the pending list.  This guarantees that our total wake
    // source count cannot change while we generate the report, and also means
    // that wake source destruction cannot invalidate our iterator out from
    // under us when we drop the pending lock in order to copy data out to
    // user-mode.
    //
    // See the extensive comment in `wake-vector.h` for a more comprehensive
    // explanation of the pending list and the various ways that operations
    // interact with it, and the things they need to do in order to remain safe.
    //
    Guard<Mutex> guard(GlobalListLock::Get());
    {
      Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};

      // Record the number of entries which are waiting to be reported at the
      // start of report generation.  We'll adjust this number later on to
      // reflect the actual number of entries which were waiting at the start of
      // the report which didn't actually fit into the user's buffer.
      hdr.unreported_wake_report_entries =
          LimitSizeT<decltype(hdr.unreported_wake_report_entries)>(pending_list_.size());

      if (do_report_entries) {
        PendingList::iterator next = pending_list_.begin();
        entry_side_buffer_len = ktl::min<size_t>(num_entries, pending_list_.size());

        // Now that we have captured our initial iterator, and computed the
        // maximum number of entries we might attempt to report, allocate a side
        // buffer we can copy the information into.  We cannot simply copy into
        // a stack allocated single entry which we then copy to user mode after
        // dropping the pending lock.  We are not allowed to hold _any_ locks
        // when we copy into user mode, so we will need to be able to buffer
        // _all_ of our results  before starting to do so.
        //
        // TODO(johngro): We don't necessarily have to do this every time we
        // generate a report.  We could also keep the allocation around as a
        // static member of WakeEvent protected by the global lock, and growing
        // it only when we needed to.  At the end of the op (when we drop the
        // global lock to copy out to user mode), we could move the buffer out,
        // perform the copy, then lock and move the buffer back in again.  We'd
        // never have a second buffer sitting around unless there was a
        // concurrent attempt to generate a report, something which is
        // technically possible but should never happen in practice.
        zx_status_t alloc_result{ZX_OK};
        pending_guard.CallUnlocked([&]() {
          if (entry_side_buffer_len > 0) {
            fbl::AllocChecker ac;
            entry_side_buffer = fbl::make_unique_checked<zx_wake_source_report_entry_t[]>(
                ac, entry_side_buffer_len);
            if (!ac.check()) {
              alloc_result = ZX_ERR_NO_MEMORY;
            }
          }
        });

        // Bail out if we could not allocate a side buffer.
        if (alloc_result != ZX_OK) {
          return alloc_result;
        }

        while (next.IsValid() && (reported_entries < entry_side_buffer_len)) {
          PendingList::iterator iter = next++;
          bool skip_reporting = false;

          if (!iter->is_signaled()) {
            // The wake source is not currently signaled.  If it has been
            // reported already, then skip adding it to the user's report.
            // Either way, remove it from the pending list.
            if (iter->has_been_reported()) {
              skip_reporting = true;
            }
            pending_list_.erase(iter);
          }

          if (!skip_reporting) {
            // Copy our result into our side buffer and mark it as having been
            // reported.  Then, then drop our lock briefly allowing pending IRQs
            // to run, and wake sources to become signaled/acked.
            entry_side_buffer[reported_entries++] = iter->report_info_;
            iter->AssignHasBeenReported(true);
            pending_guard.CallUnlocked([]() { arch::Yield(); });
          }

          // Whether we added the entry to the user's report, or skipped it
          // because the entry was just waiting around to be cleaned up by us,
          // we should drop the unreported count.
          DEBUG_ASSERT(hdr.unreported_wake_report_entries > 0);
          hdr.unreported_wake_report_entries--;
        }
      }
    }

    // Record the total number of wake sources in the system as well as our
    // report time before dropping the global lock.
    hdr.total_wake_sources = LimitSizeT<decltype(hdr.total_wake_sources)>(global_list_.size());
    hdr.report_time = current_boot_time();
  }

  // We're done.  Finish up by trying to copy everything back out to user-mode.
  if (do_report_entries) {
    DEBUG_ASSERT(reported_entries <= entry_side_buffer_len);

    if (const zx_status_t status =
            out_entries.copy_array_to_user(entry_side_buffer.get(), reported_entries, 0);
        status != ZX_OK) {
      return status;
    }

    if (const zx_status_t status = actual_entries.copy_to_user(reported_entries); status != ZX_OK) {
      return status;
    }
  }

  return out_header.copy_to_user(hdr);
}

void WakeEvent::DiscardWakeEventReport() {
  Guard<Mutex> guard(GlobalListLock::Get());
  Guard<SpinLock, IrqSave> pending_guard{PendingListLock::Get()};

  PendingList::iterator next = pending_list_.begin();
  while (next.IsValid()) {
    PendingList::iterator iter = next++;

    // Remove the event from the pending list iff it has been acknowledged.
    // Make sure the reported flag is set when we remove it from the list.
    if (!iter->is_signaled()) {
      iter->AssignHasBeenReported(true);
      pending_list_.erase(iter);
    }

    // Drop the lock and arch::Yield to give any pending triggers or acks a chance to run.
    pending_guard.CallUnlocked([]() { arch::Yield(); });
  }
}

}  // namespace wake_vector
