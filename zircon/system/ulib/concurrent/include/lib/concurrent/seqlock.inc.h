// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_SEQLOCK_INC_H_
#define LIB_CONCURRENT_SEQLOCK_INC_H_

#include <lib/concurrent/seqlock.h>

namespace concurrent {
namespace internal {

template <typename Osal, SyncOpt kSyncOpt>
typename SeqLock<Osal, kSyncOpt>::ReadTransactionToken
SeqLock<Osal, kSyncOpt>::BeginReadTransaction() const {
  SequenceNumber seq_num;

  while (((seq_num = seq_num_.load(std::memory_order_acquire)) & 0x1) != 0) {
    Osal::ArchYield();
  }

  return ReadTransactionToken{seq_num};
}

template <typename Osal, SyncOpt kSyncOpt>
bool SeqLock<Osal, kSyncOpt>::TryBeginReadTransaction(ReadTransactionToken& out_token,
                                                      zx_duration_mono_t timeout) const {
  return TryBeginReadTransactionDeadline(out_token, Osal::GetClockMonotonic() + timeout);
}

template <typename Osal, SyncOpt kSyncOpt>
bool SeqLock<Osal, kSyncOpt>::TryBeginReadTransactionDeadline(ReadTransactionToken& out_token,
                                                              zx_instant_mono_t deadline) const {
  while (((out_token.seq_num_ = seq_num_.load(std::memory_order_acquire)) & 0x1) != 0) {
    if (Osal::GetClockMonotonic() >= deadline) {
      return false;
    }
    Osal::ArchYield();
  }

  return true;
}

template <typename Osal, SyncOpt kSyncOpt>
bool SeqLock<Osal, kSyncOpt>::EndReadTransaction(ReadTransactionToken token) const {
  if ((token.seq_num_ & 0x1) != 0) {
    return false;
  }

  // If we are using fence-to-fence synchronization, this is the place we
  // need to put our acquire fence.
  if constexpr (kSyncOpt == SyncOpt::Fence) {
    std::atomic_thread_fence(std::memory_order_acquire);
  }

  return seq_num_.load(std::memory_order_relaxed) == token.seq_num_;
}

template <typename Osal, SyncOpt kSyncOpt>
void SeqLock<Osal, kSyncOpt>::Acquire() {
  while (true) {
    SequenceNumber expected;

    // Wait until we observe an even sequence number.
    while (((expected = seq_num_.load(std::memory_order_relaxed)) & 0x1) != 0) {
      Osal::ArchYield();
    }

    // Attempt to increment the even number we observed to be an odd number,
    // with Acquire semantics on the RMW if we succeed.
    if (seq_num_.compare_exchange_strong(expected, expected + 1, std::memory_order_acquire,
                                         std::memory_order_relaxed)) {
      // If we are using fence-to-fence synchronization, this is the place we
      // need to put our release fence.
      if constexpr (kSyncOpt == SyncOpt::Fence) {
        std::atomic_thread_fence(std::memory_order_release);
      }
      break;
    }
  }
}

template <typename Osal, SyncOpt kSyncOpt>
bool SeqLock<Osal, kSyncOpt>::TryAcquire(zx_duration_mono_t timeout) {
  return TryAcquireDeadline(Osal::GetClockMonotonic() + timeout);
}

template <typename Osal, SyncOpt kSyncOpt>
bool SeqLock<Osal, kSyncOpt>::TryAcquireDeadline(zx_instant_mono_t deadline) {
  while (true) {
    SequenceNumber expected;

    // Wait until we observe an even sequence number.  Bail out if we exceed our
    // deadline.
    while (((expected = seq_num_.load(std::memory_order_relaxed)) & 0x1) != 0) {
      if (Osal::GetClockMonotonic() >= deadline) {
        return false;
      }
      Osal::ArchYield();
    }

    // Attempt to increment the even number we observed to be an odd number,
    // with Acquire semantics on the RMW if we succeed.
    if (seq_num_.compare_exchange_strong(expected, expected + 1, std::memory_order_acquire,
                                         std::memory_order_relaxed)) {
      // If we are using fence-to-fence synchronization, this is the place we
      // need to put our release fence.
      if constexpr (kSyncOpt == SyncOpt::Fence) {
        std::atomic_thread_fence(std::memory_order_release);
      }
      return true;
    }

    // Make sure we check our deadline again.  We don't want to be in a
    // situation where the relaxed load (at the start of the while) always sees
    // an even number, but the subsequent CMPX always fails, causing us to never
    // check our deadline.
    if (Osal::GetClockMonotonic() >= deadline) {
      return false;
    }
  }
}

template <typename Osal, SyncOpt kSyncOpt>
void SeqLock<Osal, kSyncOpt>::Release() {
  [[maybe_unused]] SequenceNumber before = seq_num_.fetch_add(1, std::memory_order_release);
  ZX_DEBUG_ASSERT((before & 0x1) != 0);
}

}  // namespace internal
}  // namespace concurrent

#endif  // LIB_CONCURRENT_SEQLOCK_INC_H_
