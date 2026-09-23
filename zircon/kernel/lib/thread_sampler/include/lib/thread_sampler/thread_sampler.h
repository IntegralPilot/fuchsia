// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
#ifndef ZIRCON_KERNEL_LIB_THREAD_SAMPLER_INCLUDE_LIB_THREAD_SAMPLER_THREAD_SAMPLER_H_
#define ZIRCON_KERNEL_LIB_THREAD_SAMPLER_INCLUDE_LIB_THREAD_SAMPLER_THREAD_SAMPLER_H_

#include <arch.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/syscalls/sampler.h>
#include <zircon/types.h>

#include <fbl/ref_ptr.h>
#include <kernel/lockdep.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <object/thread_dispatcher.h>
#include <vm/pinned_vm_object.h>

namespace thread_sampler_tests {
class TestThreadSampler;
}  // namespace thread_sampler_tests

namespace sampler {
class ThreadSampler;

/**
 * The current state of the sampler.
 *
 * Valid state transitions are:
 *
 * ```
 *
 *    /- [ Destroying ] <- --\
 *    |                      |
 *    v                      |
 * [ Unallocated ] -> [ Configured ] -> [ Running ]
 *                             ^          |
 *                             |          v
 *                             \-[ Stopping ]
 *
 * ```
 */
enum class SamplingState : uint8_t {
  // The idle state for the sampler. We're not actively sampling nor is there a user handle
  // associated with the thread sampler singleton.
  Unallocated = 0,

  // We have buffers allocated and the user has a handle to us and can start a session. Reading is
  // allowed, but not writing.
  Configured,

  // The session is in progress. We are taking samples and writing data. Both reading and writing
  // are allowed.
  Running,

  // The session is stopping, no more write references to the buffers are allowed to be created and
  // we're waiting for existing write references to be released.
  Stopping,

  // We have stopped giving out read references and are waiting for all existing read references to
  // close. Once there are no buffer references, we'll destroy the buffers and become unallocated.
  Destroying,
};

class ThreadSampler {
 public:
  ThreadSampler() = default;
  ~ThreadSampler() = default;

  SamplingState State() const {
    return static_cast<SamplingState>(state_.load(ktl::memory_order_acquire) & kStateMask);
  }

  zx::result<> SetUp(const zx_sampler_config_t& config) TA_EXCL(ThreadSamplerLock::Get());
  zx::result<> Start() TA_EXCL(ThreadSamplerLock::Get());
  zx::result<> Stop() TA_EXCL(ThreadSamplerLock::Get());
  zx::result<> Destroy() TA_EXCL(ThreadSamplerLock::Get());

  // ReadUser calls into VmObject::ReadUser. As we could be copying to pager backed user memory, we
  // must not hold any locks.
  ktl::pair<zx_status_t, size_t> ReadUser(user_out_ptr<void> ptr, size_t len)
      TA_EXCL(ThreadSamplerLock::Get());

  class PerCpuBufferRef {
   public:
    explicit PerCpuBufferRef(ktl::atomic<uint64_t>& state, percpu_writer::Buffer& buffer)
        : buffer_(buffer), state_(state) {}
    PerCpuBufferRef(const PerCpuBufferRef&) = delete;
    PerCpuBufferRef& operator=(const PerCpuBufferRef&) = delete;
    ~PerCpuBufferRef() {
      constexpr uint64_t decrement = kBufferRefCountIncrement + kWriteRefCountIncrement;
      state_.fetch_sub(decrement, ktl::memory_order_acq_rel);
    }
    percpu_writer::Buffer& Get() { return buffer_; }

   private:
    percpu_writer::Buffer& buffer_;
    ktl::atomic<uint64_t>& state_;
  };

  // Atomically acquire a reference to the buffer for `cpu_num` and ensure that the buffers are not
  // destroyed until the reference is released.
  ktl::optional<PerCpuBufferRef> GetBufferRefForWriting(cpu_num_t cpu_num) {
    if (cpu_num >= per_cpu_buffers_.size()) {
      return ktl::nullopt;
    }
    constexpr uint64_t increment = kBufferRefCountIncrement + kWriteRefCountIncrement;
    uint64_t expected = state_.load(ktl::memory_order_relaxed);
    bool success = false;
    do {
      const SamplingState state = static_cast<SamplingState>(expected & kStateMask);
      if (state != SamplingState::Running) {
        return ktl::nullopt;
      }
      if (((expected & kBufferRefCountMask) == kBufferRefCountMask) ||
          ((expected & kWriteRefCountMask) == kWriteRefCountMask)) {
        // This shouldn't happen, but we should handle it release builds regardless.
        DEBUG_ASSERT((expected & kBufferRefCountMask) != kBufferRefCountMask);
        DEBUG_ASSERT((expected & kWriteRefCountMask) != kWriteRefCountMask);
        return ktl::nullopt;
      }
      const uint64_t desired = expected + increment;
      success = state_.compare_exchange_weak(expected, desired, ktl::memory_order_acq_rel,
                                             ktl::memory_order_relaxed);
    } while (!success);
    return ktl::make_optional<PerCpuBufferRef>(state_, per_cpu_buffers_[cpu_num]);
  }

  struct ReadToken {
   public:
    ReadToken(ktl::atomic<uint64_t>& state, fbl::Array<percpu_writer::Buffer>& buffers)
        : buffers_(buffers), state_(state) {}
    fbl::Array<percpu_writer::Buffer>& Get() { return buffers_; }
    ReadToken(const ReadToken&) = delete;
    ReadToken& operator=(const ReadToken&) = delete;
    ~ReadToken() { state_.fetch_sub(kBufferRefCountIncrement, ktl::memory_order_acq_rel); }

   private:
    fbl::Array<percpu_writer::Buffer>& buffers_;
    ktl::atomic<uint64_t>& state_;
  };

  // Atomically acquire a reference to the buffer for `cpu_num` and ensure that the buffers are not
  // destroyed until the reference is released.
  ktl::optional<ReadToken> GetBufferRefForReading() TA_EXCL(ThreadSamplerLock::Get()) {
    uint64_t expected = state_.load(ktl::memory_order_relaxed);
    bool success = false;
    do {
      const SamplingState state = static_cast<SamplingState>(expected & kStateMask);
      if (state == SamplingState::Unallocated || state == SamplingState::Destroying) {
        return ktl::nullopt;
      }
      if ((expected & kBufferRefCountMask) == kBufferRefCountMask) {
        // This shouldn't happen, but we should handle it release builds regardless.
        DEBUG_ASSERT((expected & kBufferRefCountMask) != kBufferRefCountMask);
        return ktl::nullopt;
      }
      const uint64_t desired = expected + kBufferRefCountIncrement;
      success = state_.compare_exchange_weak(expected, desired, ktl::memory_order_acq_rel,
                                             ktl::memory_order_relaxed);
    } while (!success);
    return ktl::make_optional<ReadToken>(state_, per_cpu_buffers_);
  }

  // Atomically request the current cpu mark a thread for sampling if the session is Running.
  void ScheduleMarking();

  // Atomically rerequest the current cpu mark a thread for sampling if the session is Running.
  // Avoids modifying the ref count unless the session has stopped.
  void RescheduleMarking();

  // Atomically cancel a request that the current cpu mark a thread for sampling.
  void CancelMarking();

  // Given information about a thread and its registers, walk its userstack and write out a sample
  // if sampling is enabled.
  zx::result<> SampleThread(zx_koid_t pid, zx_koid_t tid, GeneralRegsSource source,
                            const void* gregs, uint64_t session_id)
      TA_EXCL(ThreadSamplerLock::Get());

 private:
  friend class ::thread_sampler_tests::TestThreadSampler;
  DECLARE_SINGLETON_MUTEX(ThreadSamplerLock);
  void SetState(SamplingState new_state) TA_REQ(ThreadSamplerLock::Get()) {
    // While the SamplingState component of `state_` won't change out from under us as we require a
    // mutex to change it, the writes in flight counter could change, so we use a cmpxchg loop to
    // avoid losing a buffer ref count increment or decrement.
    uint64_t expected = state_.load(ktl::memory_order_relaxed);
    bool success = false;
    do {
      const uint64_t desired = (expected & ~(kStateMask)) | static_cast<uint64_t>(new_state);
      success = state_.compare_exchange_weak(expected, desired, ktl::memory_order_acq_rel,
                                             ktl::memory_order_relaxed);
    } while (!success);
  }

  void StopLocked() TA_REQ(ThreadSamplerLock::Get());

  // per_cpu_buffers_ and the SamplingState bits of state_ may be READ without acquiring the
  // ThreadSamplerLock. However, the lock must be acquired to WRITE them.
  //
  // per_cpu_buffers_ must not be modified while the session is in the states:
  //  - Configured
  //  - Running
  //  - Stopping
  // state_ is eight bytes composed as:
  //
  // MM MM WW WW BB BB XX SS
  //
  // SS: 8 bits, SamplingState
  // XX: 8 bits, Reserved
  // BB: 16 bits, BufferRefCount
  // WW: 16 bits, WriteRefCount
  // MM: 16 bits, MarkingsScheduled
  //
  // Rules for our state:
  //
  // BufferRefCount is used to control when we can transition from Configured to Unallocated.
  // BufferRefCount is incremented when a caller reads data through zx_sampler_read.
  // - BufferRefCount must be zero if State is Unallocated.
  // - BufferRefCount can only be incremented in states Configured, Stopping, and Running
  // - We use the intermediate state Destroying to prevent new references and wait for existing
  //   references to close.
  // - We cannot transition to Unallocated until BufferRefCount is 0
  //
  // WriteRefCount is used to control when we can transition from Running to Configured.
  // WriteRefCount is incremented before a writer attempts to access any per-cpu buffer, and
  // decremented when it’s done writing.
  // - WriteRefCount must be zero if State is Unallocated, Configured, or Destroying.
  // - WriteRefCount can only be incremented in the state Running
  // - We use the intermediate state Stopping to prevent new Write references and wait for existing
  //   ones to close.
  // - We cannot transition from Running to Configured until WriteRefCount is 0.
  // - As we always require a BufferRef in order to write, it will always be that BufferRef >=
  //   WriteRefCount.
  //
  // MarkingsScheduled is also used to control when we can transition from Running to Configured.
  // MarkingsScheduled is equal to the number of timer callbacks running or scheduled to run. It is
  // decremented when we cancel the timer callbacks or a timer callback finds that the state is no
  // longer Running.
  // - MarkingsScheduled must be zero if State is Unallocated, Configured, or Destroying.
  // - MarkingScheduled can only be incremented in the state Running.
  // - We use the intermediate state Stopping to prevent new timers and wait for existing ones to
  //   close.
  // - We cannot transition from Running to Configured until MarkingsScheduled is 0.
  static constexpr uint64_t kStateMaskShift = 0;
  static constexpr uint64_t kStateMask = 0xFF << kStateMaskShift;

  static constexpr uint64_t kBufferRefCountShift = 16;
  static constexpr uint64_t kBufferRefCountIncrement = uint64_t{1} << kBufferRefCountShift;
  static constexpr uint64_t kBufferRefCountMask = uint64_t{0xFFFF} << kBufferRefCountShift;

  static constexpr uint64_t kWriteRefCountShift = 32;
  static constexpr uint64_t kWriteRefCountIncrement = uint64_t{1} << kWriteRefCountShift;
  static constexpr uint64_t kWriteRefCountMask = uint64_t{0xFFFF} << kWriteRefCountShift;

  static constexpr uint64_t kMarkingsScheduledShift = 48;
  static constexpr uint64_t kMarkingsScheduledIncrement = uint64_t{1} << kMarkingsScheduledShift;
  static constexpr uint64_t kMarkingsScheduledMask = uint64_t{0xFFFF} << kMarkingsScheduledShift;

  ktl::atomic<uint64_t> state_ = 0;

  // The current session. Monotonically increasing.
  //
  // Used to prevent scheduled markings from being so delayed that they trigger in
  // a subsequent session by marking threads which which session_id their next sample is intended
  // for.
  RelaxedAtomic<uint64_t> session_id_ = 0;

  fbl::Array<percpu_writer::Buffer> per_cpu_buffers_{nullptr};
  zx_duration_t sample_period_{0};
};

extern ThreadSampler gThreadSampler;

// Joins the sampling session on the current cpu if one exists.
void sampler_percpu_init();

// Exit the sampling session on the current cpu if one exists.
void sampler_percpu_shutdown();

}  // namespace sampler

#endif  // ZIRCON_KERNEL_LIB_THREAD_SAMPLER_INCLUDE_LIB_THREAD_SAMPLER_THREAD_SAMPLER_H_
