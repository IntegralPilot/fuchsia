// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/fit/defer.h>
#include <lib/fxt/serializer.h>
#include <lib/thread_sampler/thread_sampler.h>
#include <lib/zx/time.h>

#include <fbl/array.h>
#include <kernel/dpc.h>
#include <kernel/event.h>
#include <kernel/mp.h>
#include <kernel/spinlock.h>
#include <lk/init.h>
#include <object/io_buffer_dispatcher.h>
#include <object/process_dispatcher.h>

// We have only a single global thread sampler at a time. Another callers will get
// ZX_ERR_ALREADY_EXISTS until the existing sampler is released.
namespace sampler {
sampler::ThreadSampler gThreadSampler{};

void sampler_percpu_init() { gThreadSampler.ScheduleMarking(); }
void sampler_percpu_shutdown() { gThreadSampler.CancelMarking(); }
}  // namespace sampler

zx::result<> sampler::ThreadSampler::SetUp(const zx_sampler_config_t& config) {
  fbl::Array<percpu_writer::Buffer> per_cpu_buffers;
  Guard<Mutex> guard(ThreadSamplerLock::Get());
  SamplingState state = State();

  if (state != SamplingState::Unallocated) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  if (config.period == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const size_t num_cpus = percpu::processor_count();
  if (!per_cpu_buffers_) {
    // Perform the allocations for the state without the lock held as this may potentially block
    // waiting for memory.
    zx::result<> result;
    guard.CallUnlocked([&]() {
      fbl::AllocChecker ac;
      per_cpu_buffers = fbl::MakeArray<percpu_writer::Buffer>(&ac, num_cpus);
      if (!ac.check()) {
        result = zx::error(ZX_ERR_NO_MEMORY);
        return;
      }

      // Even though the buffer is per_cpu, we are fine to set up each cpu state here on a single
      // cpu. When we start sampling, we call mp_sync_exec which will synchronize the written
      // per_cpu_buffers.
      for (size_t i = 0; i < num_cpus; i++) {
        zx_status_t init_res =
            per_cpu_buffers[i].Init(static_cast<uint32_t>(config.buffer_size), "sampler",
                                    fxt::ThreadRef{fxt::Koid{0}, fxt::Koid{i}});
        if (init_res != ZX_OK) {
          result = zx::error(init_res);
          return;
        }
      }
    });
    // Propagate any errors.
    if (result.is_error()) {
      return result;
    }
    // Reload and check state again as it may have changed while the lock was dropped.
    state = State();
    if (state != SamplingState::Unallocated) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
  }
  // Re-check whether there if per_cpu_buffers_ exists as this may have changed while the lock was
  // dropped. If we raced and someone else allocated state before us this is fine and we will just
  // drop the local allocation.
  if (per_cpu_buffers_) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  ASSERT(per_cpu_buffers);

  per_cpu_buffers_ = ktl::move(per_cpu_buffers);
  sample_period_ = config.period;
  SetState(SamplingState::Configured);
  return zx::ok();
}

zx::result<> sampler::ThreadSampler::Start() {
  Guard<Mutex> guard(ThreadSamplerLock::Get());
  if (State() != SamplingState::Configured) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  DEBUG_ASSERT(!per_cpu_buffers_.empty());

  SetState(SamplingState::Running);
  mp_sync_exec(mp_ipi_target::ALL, 0, [](void*) { gThreadSampler.ScheduleMarking(); }, nullptr);
  return zx::ok();
}

zx::result<> sampler::ThreadSampler::Stop() {
  Guard<Mutex> guard(ThreadSamplerLock::Get());
  if (State() != SamplingState::Running) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  StopLocked();
  return zx::ok();
}

void sampler::ThreadSampler::StopLocked() {
  // We start by disabling new writes, timers, and buffer write references. Then we need to wait for
  // any that are in flight to finish.
  SetState(SamplingState::Stopping);

  session_id_.fetch_add(1);
  // We could be racing with the timer callback here. If the timer callback is currently executing,
  // we could end up in a state where the timer is canceled, but we don't reset the kPendingTimer
  // bits. As the timer callback occurs with interrupts disabled, we cancel the timers on the CPUs
  // they trigger to serialize the cancelation with a potential callback.
  mp_sync_exec(mp_ipi_target::ALL, 0, [](void*) { gThreadSampler.CancelMarking(); }, nullptr);

  // Some timers may not have not been able to be canceled, so we need to wait for any samples that
  // have already started to finish.
  constexpr zx_duration_t warn_duration = ZX_SEC(30);
  zx_duration_t sleep_duration = ZX_MSEC(1);
  zx_instant_mono_t next_warn_time = zx_time_add_duration(current_mono_time(), warn_duration);
  int64_t warn_events = 0;
  constexpr zx_duration_t max_sleep_duration = ZX_SEC(1);

  // Stopping ends the session, but keeps the buffers alive so reads may exfiltrate any remaining
  // data. Here, we need to wait for all writes and markings to finish.
  const uint64_t pending_operations_mask = kWriteRefCountMask | kMarkingsScheduledMask;
  while (state_.load(ktl::memory_order_acquire) & pending_operations_mask) {
    // Warn if we have spend an 'unreasonable' amount of time waiting.
    if (current_mono_time() > next_warn_time) {
      warn_events++;
      printf("WARNING: Waited more than %ld seconds for sampling to finish\n",
             (warn_events * warn_duration) / ZX_SEC(1));
      next_warn_time = zx_time_add_duration(next_warn_time, warn_duration);
    }
    Thread::Current::SleepRelative(sleep_duration);
    // Scale up the sleep duration to balance being initially responsive and not consuming
    // excessive CPU.
    sleep_duration = ktl::min(sleep_duration * 2, max_sleep_duration);
  }

  // At this point, there are no longer pending writes or marks. There may still be threads that
  // have been marked to be sampled, but have not yet attempted a sample. In addition, there may
  // still be buffer read references.
  //
  // As the sampler has static lifetime, these threads can safely access `state_` and will see that
  // the session is no longer running, aborting their attempt at a sample.
  SetState(SamplingState::Configured);
}

zx::result<> sampler::ThreadSampler::Destroy() {
  Guard<Mutex> guard(ThreadSamplerLock::Get());
  if (State() == SamplingState::Running) {
    // The userspace end of the sampler has closed and we've skipped right to destroying.
    StopLocked();
  }
  SetState(SamplingState::Destroying);

  // Some timers may not have not been able to be canceled, so we need to wait for any samples that
  // have already started to finish.
  //
  // In order to destroy the buffers, we should have no references and no in flight samples.
  // After StopLocked, the WriteRefCount and SamplingScheduled must be 0, and can no longer
  // be incremented. So we only now need to wait for all buffer references to close.
  zx_instant_mono_t deadline = zx_time_add_duration(current_mono_time(), ZX_SEC(30));
  const uint64_t pending_operations_mask = kBufferRefCountMask;
  uint64_t pending_operations;
  do {
    pending_operations = state_.load(ktl::memory_order_acquire) & pending_operations_mask;
    if (pending_operations) {
      Thread::Current::SleepRelative(ZX_MSEC(1));
    }
  } while (pending_operations && (current_mono_time() < deadline));
  // We'll wait an unreasonable amount of time for the operations to finish. If the operations
  // really haven't finished by this point, something has gone terribly wrong.
  if (pending_operations) {
    printf("WARNING: Timed out after waiting 30 seconds for sampler destruction\n");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // After StopLocked, we have prevented further threads from accessing the per_cpu_states, and then
  // waited for any threads that were accessing the states to finish.
  //
  // It's now safe to destroy our cpu states. This will destroy the mappings and pinnings that the
  // kernel keeps to write to.
  SetState(SamplingState::Unallocated);
  per_cpu_buffers_.reset();

  return zx::ok();
}

zx::result<> sampler::ThreadSampler::SampleThread(zx_koid_t pid, zx_koid_t tid,
                                                  GeneralRegsSource source, const void* gregs,
                                                  uint64_t session_id) {
  // We are going to attempt a usercopy below which might fault, so interrupts cannot be disabled.
  DEBUG_ASSERT(!arch_ints_disabled());
  // We need to be a little bit careful here because we could be racing with a Stop operation. The
  // Stop operation:
  //
  // 1) Disables Writes
  // 2) Cancels each Timer
  // 3) Waits for all PendingWrites to finish
  //
  // It does this while holding the ThreadSamplerDispatcher lock. This means if SetPendingWrite and
  // then attempt to obtain the ThreadSamplerDispatcher lock, we could deadlock.
  //
  // Instead, we'll do a single enabled check here before attempting to read the stack, which will
  // take some time. Once we've collected our data and are ready to write out, we'll
  // SetPendingWrite to hold onto the buffers for the duration of the write.
  //
  // If we find that writes are enabled, we are safe to write to the buffers as
  // Stop will not destroy them until we lower the PendingWrite bit.
  //
  // If we find that writes are disabled, we throw away our sample as it's no longer safe to write
  // to the buffers.
  if (State() != SamplingState::Running) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // If the session ids don't match, this sample request is one that got _way_ delayed from a
  // previous session.
  if (session_id != session_id_.load()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  size_t frame_num = 0;
  constexpr size_t kMaxUserBacktraceSize = 64;
  // We're dropping 512 bytes on the kernel stack here and we need a be careful not to overflow it.
  //
  // This amount of bytes _should_ be safe because SampleThread is only called during
  // Thread::Current::ProcessPendingSignals which occurs directly before returning to usermode. At
  // this point, the stack will be shallow.
  vaddr_t bt[kMaxUserBacktraceSize]{};

  vaddr_t fp = 0;
  vaddr_t pc = 0;
  switch (source) {
    case GeneralRegsSource::None:
      break;
    case GeneralRegsSource::Iframe:
#ifdef __x86_64__
      fp = reinterpret_cast<const iframe_t*>(gregs)->rbp;
      pc = reinterpret_cast<const iframe_t*>(gregs)->ip;
#endif
#ifdef __aarch64__
      fp = reinterpret_cast<const iframe_t*>(gregs)->r[29];
      pc = reinterpret_cast<const iframe_t*>(gregs)->elr - 4;
#endif
#ifdef __riscv
      fp = reinterpret_cast<const iframe_t*>(gregs)->regs.s0;
      pc = reinterpret_cast<const iframe_t*>(gregs)->regs.pc;
#endif
      break;
#ifdef __x86_64__
    case GeneralRegsSource::Syscall:
      fp = reinterpret_cast<const syscall_regs_t*>(gregs)->rbp;
      pc = reinterpret_cast<const syscall_regs_t*>(gregs)->rip;
      break;
#endif
  }

  if (pc == 0) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  bt[frame_num++] = pc;

  while (frame_num < kMaxUserBacktraceSize) {
    vaddr_t actual_fp = fp;
    if (fp == 0) {
      // We've reached the top of the frame pointer chain.
      break;
    }

    // RISC-V has a nonstandard frame pointer which points to the CFA instead of
    // the previous frame pointer. Since the frame pointer and return address are
    // always just below the CFA, subtract 16 bytes to get to the actual frame pointer.
#if __riscv
    actual_fp -= 16;
#endif

    user_in_ptr<const vaddr_t> user_next_fp{reinterpret_cast<vaddr_t*>(actual_fp)};
    user_in_ptr<const vaddr_t> user_pc{reinterpret_cast<vaddr_t*>(actual_fp + 8)};

    // A well formed frame pointer chain ends in 0 and should never fail to copy. If a thread's
    // stack is not readable or well formatted, we return an error to indicate that sampling should
    // be disabled for the offending thread.
    zx_status_t copy_res = user_pc.copy_from_user(&pc);
    if (copy_res != ZX_OK) {
      // We eat the copy_res and return ZX_ERR_NOT_SUPPORTED here and below to indicate that we
      // failed to take a sample, but we might still succeed in the future. A thread may not
      // necessarily have valid frame pointers at all points in execution, so don't give on this
      // thread just yet.
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    if (pc == 0) {
      break;
    }
    bt[frame_num++] = pc;
    copy_res = user_next_fp.copy_from_user(&fp);
    if (copy_res != ZX_OK) {
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
  }
  // Up until this point, interrupts are enabled so that we can handle faults when doing usercopies.
  // However, once we want to write, we need to disable interrupts as underlying buffer writing
  // algorithm assumes a single writer. We ensure we don't get interrupted or context switched while
  // we are writing. Otherwise, we could get context switched out, and then have another thread
  // attempt to write.
  InterruptDisableGuard irqd;

  ktl::optional<PerCpuBufferRef> token = GetBufferRefForWriting(arch_curr_cpu_num());
  if (!token) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  percpu_writer::Buffer& cpu_state = token->Get();
  const fxt::ThreadRef current_thread{pid, tid};

  // Drop the record if we fail to write out.
  zx_status_t _ = fxt::WriteProfilerBacktraceRecord(
      &cpu_state, current_mono_ticks(), current_thread, bt, sizeof(uint64_t) * frame_num);
  return zx::ok();
}

ktl::pair<zx_status_t, size_t> sampler::ThreadSampler::ReadUser(user_out_ptr<void> ptr,
                                                                size_t len) {
  // We unfortunately run into some complexity here: while the buffer our samples in is created by
  // the kernel and is safe to read from, the user memory we are writing to could be pager-backed.
  // This means that when we attempt to write to it as part of the VmObjectPaged::ReadUser call,
  // we cannot be holding locks.
  //
  // During the copy, we'd need to prevent:
  //   1) The buffers being destroyed due to the read handle being zx_handle_close'd
  //   2) A new sampler from being created.
  //
  // We do this by atomically incrementing a ref count on the buffers if they are safe to
  // read, then only allowing the buffers to be destroyed and the sampler state to advance once we
  // release the ref count.
  lockdep::AssertNoLocksHeld();

  ktl::optional<ReadToken> ref = GetBufferRefForReading();
  if (!ref) {
    return {ZX_ERR_BAD_STATE, 0};
  }
  fbl::Array<percpu_writer::Buffer>& per_cpu_buffers = ref->Get();

  const size_t num_buffers = per_cpu_buffers.size();
  // All buffers are the same size.
  const size_t buffer_size = per_cpu_buffers[0].Size();

  // The caller can query the required buffer size by passing in a nulltpr.
  if (!ptr) {
    return {ZX_OK, buffer_size * num_buffers};
  }

  // Eventually, this should support users passing in buffers smaller than the sum of the size of
  // all per-CPU buffers, but for now we do not allow this.
  if (len < (buffer_size * num_buffers)) {
    return {ZX_ERR_INVALID_ARGS, 0};
  }

  // Iterate through each per-CPU buffer and read its contents.
  size_t bytes_read = 0;
  user_out_ptr<ktl::byte> byte_ptr = ptr.reinterpret<ktl::byte>();

  auto copy_fn = [&](uint32_t byte_offset, ktl::span<ktl::byte> src) {
    // This is safe to do while holding the lock_ because the KTrace lock is a leaf lock that is
    // not acquired during the course of a page fault.
    zx_status_t status = ZX_ERR_BAD_STATE;
    // Compute the destination address for this segment.
    user_out_ptr out_ptr = byte_ptr.byte_offset(bytes_read + byte_offset);

    // Copy the trace data to the user segment.
    status = out_ptr.copy_array_to_user(src.data(), src.size());
    return status;
  };

  for (uint32_t i = 0; i < num_buffers; i++) {
    const zx::result<size_t> result = per_cpu_buffers[i].Read(copy_fn, static_cast<uint32_t>(len));
    if (result.is_error()) {
      // If we copied some data from a previous buffer, we have to return the fact that we did so
      // here. Otherwise, that data will be lost.
      return {result.status_value(), bytes_read};
    }
    bytes_read += result.value();
  }
  return {ZX_OK, bytes_read};
}

void sampler::ThreadSampler::ScheduleMarking() {
  DEBUG_ASSERT(arch_ints_disabled());

  // sampler_percpu_init and an IPI from  ThreadSampler::Start IPI might race if we attempt to Start
  // a session while a core is being onlined. We handle this race by clearing our state out first
  // then resetting it.
  //
  // CancelMarking is safe to call even if no timer is set -- it'll only decrement the refcount if a
  // timer was actually canceled.
  CancelMarking();

  uint64_t expected = state_.load();
  bool success;
  // We only want to schedule a sample while we are running.
  do {
    SamplingState s = static_cast<SamplingState>(expected & kStateMask);
    if (s != SamplingState::Running) {
      return;
    }
    if ((expected & kMarkingsScheduledMask) == kMarkingsScheduledMask) {
      DEBUG_ASSERT((expected & kMarkingsScheduledMask) != kMarkingsScheduledMask);
      return;
    }
    const uint64_t desired = expected + kMarkingsScheduledIncrement;
    success = state_.compare_exchange_weak(expected, desired, ktl::memory_order_acq_rel,
                                           ktl::memory_order_relaxed);
  } while (!success);

  Deadline deadline = Deadline::after_mono(sample_period_);
  uint64_t session_id = session_id_.load();

  percpu::GetCurrent().sampling_timer.Set(deadline, Thread::SignalSampleStack,
                                          reinterpret_cast<void*>(session_id));
}

void sampler::ThreadSampler::RescheduleMarking() {
  DEBUG_ASSERT(arch_ints_disabled());
  uint64_t expected = state_.load(ktl::memory_order_acquire);
  SamplingState s = static_cast<SamplingState>(expected & kStateMask);
  if (s != SamplingState::Running) {
    uint64_t result = state_.fetch_sub(kMarkingsScheduledIncrement, ktl::memory_order_acq_rel);
    DEBUG_ASSERT((result & kMarkingsScheduledMask) != 0);
    return;
  }
  Deadline deadline = Deadline::after_mono(sample_period_);
  uint64_t session_id = session_id_.load();
  percpu::GetCurrent().sampling_timer.Set(deadline, Thread::SignalSampleStack,
                                          reinterpret_cast<void*>(session_id));
}

void sampler::ThreadSampler::CancelMarking() {
  DEBUG_ASSERT(arch_ints_disabled());
  bool cancelled = percpu::GetCurrent().sampling_timer.Cancel();
  if (cancelled) {
    uint64_t prev = state_.fetch_sub(kMarkingsScheduledIncrement);
    DEBUG_ASSERT((prev & kMarkingsScheduledMask) != 0);
  }
}
