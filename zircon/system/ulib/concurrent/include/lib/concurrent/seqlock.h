// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_SEQLOCK_H_
#define LIB_CONCURRENT_SEQLOCK_H_

#include <lib/concurrent/common.h>
#include <lib/concurrent/seqlock_payload.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <atomic>

namespace concurrent {
namespace internal {

template <typename Osal, SyncOpt _kSyncOpt = SyncOpt::AcqRelOps>
class __TA_CAPABILITY("mutex") SeqLock {
 public:
  static constexpr SyncOpt kSyncOpt = _kSyncOpt;
  static constexpr SyncOpt kCopyWrapperSyncOpt =
      (kSyncOpt == SyncOpt::AcqRelOps) ? SyncOpt::AcqRelOps : SyncOpt::None;

  using SequenceNumber = uint32_t;

  class ReadTransactionToken {
   public:
    ReadTransactionToken() : seq_num_(1) {}
    SequenceNumber seq_num() const { return seq_num_; }

   private:
    friend class SeqLock;
    explicit ReadTransactionToken(SequenceNumber num) : seq_num_(num) {}
    SequenceNumber seq_num_;
  };

  SeqLock() {
    static_assert((kSyncOpt == SyncOpt::AcqRelOps) || (kSyncOpt == SyncOpt::Fence),
                  "The synchronization options chosen for a SeqLock must be "
                  "either Acquire/Release, or Fence");
  }
  ~SeqLock() = default;

  // No copy, no move
  SeqLock(const SeqLock&) = delete;
  SeqLock(SeqLock&&) = delete;
  SeqLock& operator=(const SeqLock&) = delete;
  SeqLock& operator=(SeqLock&&);

  // Provide read access to the current seq_num state.  Mostly for testing.
  SequenceNumber seq_num(std::memory_order order = std::memory_order_relaxed) const {
    return seq_num_.load(order);
  }

  // Read Transactions (eg; "locking" for read)
  ReadTransactionToken BeginReadTransaction() const __TA_ACQUIRE_SHARED();
  bool TryBeginReadTransaction(ReadTransactionToken& out_token,
                               zx_duration_mono_t timeout = 0) const __TA_TRY_ACQUIRE_SHARED(true);
  bool TryBeginReadTransactionDeadline(ReadTransactionToken& out_token,
                                       zx_instant_mono_t deadline) const
      __TA_TRY_ACQUIRE_SHARED(true);
  bool EndReadTransaction(ReadTransactionToken token) const __TA_RELEASE_SHARED();

  // Exclusive locking.
  void Acquire() __TA_ACQUIRE();
  bool TryAcquire(zx_duration_mono_t timeout = 0) __TA_TRY_ACQUIRE(true);
  bool TryAcquireDeadline(zx_instant_mono_t deadline) __TA_TRY_ACQUIRE(true);
  void Release() __TA_RELEASE();

 private:
  std::atomic<SequenceNumber> seq_num_{0};
};
}  // namespace internal

#if defined(__Fuchsia__) && !defined(_KERNEL)
////////////////////////
// Fuchsia User Mode
////////////////////////
namespace internal {
struct FuchsiaUserModeOsal;
}

template <SyncOpt kSyncOpt = SyncOpt::AcqRelOps>
using SeqLock = internal::SeqLock<internal::FuchsiaUserModeOsal, kSyncOpt>;
#else
////////////////////////
// POSIX fallback
////////////////////////
namespace internal {
struct PosixUserModeOsal;
}
template <SyncOpt kSyncOpt = SyncOpt::AcqRelOps>
using SeqLock = internal::SeqLock<internal::PosixUserModeOsal, kSyncOpt>;
#endif

}  // namespace concurrent

#endif  // LIB_CONCURRENT_SEQLOCK_H_
