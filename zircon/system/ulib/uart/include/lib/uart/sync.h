// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_SYNC_H_
#define LIB_UART_SYNC_H_

#include <lib/arch/intrin.h>
#include <lib/zircon-internal/thread_annotations.h>

namespace uart {

// Degenerate case of unsynchronized policy, that provides the expected API to be implemented when
// another policy needs to be used.
struct UnsynchronizedPolicy {
  // The |Lock| consists of any environment specific capability providing synchronization
  // primitives.
  //  * |Lock| must be default-constructible.
  //  * |Lock| acquisition and release is private to the declared |Guard|.
  //  * |Lock| must model a version of TA Capabilities.
  //  * |MemberOf| is the containing type where |Lock| is embedded as a member.
  template <typename MemberOf>
  struct TA_CAP("uart") Lock {
    constexpr Lock() = default;
  };

  // The |Guard| consists of any environment specific scoped capability that presents itself
  // as a RAII for acquiring the opaque |LockType|.
  //  * |Guard| must be constructible from (|Lock*|, const char* |id|).
  //  * |LockPolicy| is forwarded to |Guard| type.
  template <typename LockPolicy>
  class TA_SCOPED_CAP Guard {
   public:
    template <typename LockType>
    Guard(LockType* lock, const char* tag) TA_ACQ(lock) {}
    ~Guard() TA_REL() {}
  };

  // The |Waiter| consists of any environment specific object, that provides a mechanism for
  // waiting for an event to happen.
  //   * |Waiter| must implement |template<typename Guard, T> Wait(Guard&, T&&
  //   enable_tx_interrupt)|.
  //
  // While the library only requires |Wait| to be implemented, users should provide a wake mechanism
  // in blocking implementations, such that observers stuck waiting can resume their work.
  //
  // |Wait| is guaranteed to be called while |guard| holds the underlying capability.
  struct Waiter {
    template <typename Guard, typename T>
    void Wait(Guard& guard, T&& enable_tx_interrupt) TA_REQ(guard) {
      arch::Yield();
    }
  };

  // Default Lock Policy to be used.
  // The meaning of the policy is only meaningful to the |Guard| or this |SyncPolicy|.
  struct DefaultLockPolicy {};

  // Delegate for Lock Holding assertions.
  template <typename LockType>
  static void AssertHeld(LockType& lock) TA_ASSERT(lock) {}
};

}  // namespace uart

#endif  // LIB_UART_SYNC_H_
