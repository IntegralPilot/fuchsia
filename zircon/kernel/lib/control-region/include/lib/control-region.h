// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_CONTROL_REGION_INCLUDE_LIB_CONTROL_REGION_H_
#define ZIRCON_KERNEL_LIB_CONTROL_REGION_INCLUDE_LIB_CONTROL_REGION_H_

#include <lib/relaxed_atomic.h>

#include <cstdint>

#include <kernel/spinlock.h>
#include <ktl/atomic.h>

// ControlRegion is a CPU synchronization tool.
//
// Think of it as a region of a program that is protected by a gate.  When the gate is open, CPUs
// may freely enter or leave the controlled region.  The gate may only be closed when all CPUs are
// within the controlled region.  The last CPU to enter the region is responsible for closing the
// gate.  CPUs may not leave the region until the gate is opened by one of them.
//
// Usage:
//
//   // An instance shared by all CPUs.
//   ControlRegion region;
//
//   // A managed resource of some kind, shared by all CPUs.
//   Resource resource;
//
//   ...
//
//   // Called once for each CPU before any other use of the region.
//   // Be sure to |Unregister| when taking a CPU offline.
//   region.Register();
//
//   ...
//
//   resource.Use();
//
//   if (!region.TryEnter()) {
//     // We are the last one in, shutdown the resource before closing and entering.
//     resource.Shutdown();
//     region.CloseEnter();
//   }
//
//   // Start of the region.
//
//   // End of the region.
//
//   if (!region.TryLeave()) {
//     // We are the first leave, we must restore the resource before opening and leaving.
//     resource.Init();
//     region.OpenLeave();
//   }
//
//  resource.Use();
//
class ControlRegion {
 public:
  // Registers a CPU for participation in a control region.
  //
  // This should be called once for each CPU that may participate.
  void Register() TA_EXCL(lock_) {
    Guard<SpinLock, IrqSave> guard{&lock_};
    DEBUG_ASSERT_MSG(state_.load() == State::kOpen, "state=%u, count=%u",
                     static_cast<uint32_t>(state_.load()), count_);
    // See note at definition of |count_|.
    count_ += 1;
  }

  // Unregisters a CPU for participation in a control region.
  //
  // This method should be called before offlining a CPU that had been registered.
  void Unregister() TA_EXCL(lock_) {
    Guard<SpinLock, IrqSave> guard{&lock_};
    DEBUG_ASSERT_MSG(state_.load() == State::kOpen && count_ > 0, "state=%u, count=%u",
                     static_cast<uint32_t>(state_.load()), count_);
    // See note at definition of |count_|.
    count_ -= 1;
  }

  // Attempt to enter the region.
  //
  // Returns true if this CPU has successfully entered and isn't the last one in.
  //
  // Returns false and sets the state to closing if this CPU is the only remaining CPU outside the
  // region.  This CPU is then obligated to first perform any resource management tasks associated
  // with this region, and then finish closing the gate and entering by calling |CloseEnter|.
  //
  // It is an error to call this method with interrupts enabled.
  [[nodiscard]] bool TryEnter() TA_EXCL(lock_) {
    Guard<SpinLock, NoIrqSave> guard{&lock_};
    DEBUG_ASSERT_MSG(state_.load() == State::kOpen && count_ > 0, "state=%u, count=%u",
                     static_cast<uint32_t>(state_.load()), count_);
    if (count_ > 1) {
      count_ -= 1;
      return true;
    }
    // We're the last one.
    state_.store(State::kClosing);
    return false;
  }

  // Close the gate and enter the region.  This is intended to be called after a call to |TryEnter|
  // has returned false and the caller has performed any necessary resource management tasks.
  //
  // It is an error to call this method with interrupts enabled.
  void CloseEnter() TA_EXCL(lock_) {
    Guard<SpinLock, NoIrqSave> guard{&lock_};
    DEBUG_ASSERT_MSG(state_.load() == State::kClosing && count_ == 1, "state=%u, count=%u",
                     static_cast<uint32_t>(state_.load()), count_);
    state_.store(State::kClosed);
    count_ -= 1;
  }

  // Attempt to leave the region.
  //
  // Returns true if this CPU has successfully left the region and there is already at least one CPU
  // outside the region.
  //
  // Returns false and sets the state to opening if no other CPUs are outside the region.  This CPU
  // is then obligated to then perform any resource management tasks associated with the region, and
  // then finish opening the gate and leaving by calling |OpenLeave|.
  //
  // It is an error to call this method with interrupts enabled.
  //
  // Note: If another CPU is in the process of opening or closing the gate, this method may spin
  // (busy-wait) until that other CPU has finished.
  bool TryLeave() TA_EXCL(lock_) {
    while (true) {
      {
        Guard<SpinLock, NoIrqSave> guard{&lock_};
        const State state = state_.load();
        switch (state) {
          case State::kOpen:
            DEBUG_ASSERT_MSG(count_ > 0, "count=%u", count_);
            count_ += 1;
            return true;

          case State::kClosed:
            // We must be the first to leave.  Start opening the gate.
            DEBUG_ASSERT_MSG(count_ == 0, "count=%u", count_);
            state_.store(State::kOpening);
            return false;

          case State::kClosing:
            DEBUG_ASSERT_MSG(count_ == 1, "count=%u", count_);
            break;

          case State::kOpening:
            DEBUG_ASSERT_MSG(count_ == 0, "count=%u", count_);
            break;

          default:
            panic("unexpected value %u", static_cast<uint32_t>(state));
        };
      }
      // Notice in the cases above where we observed that the state is either closing or opening, we
      // drop the lock and wait (spin) for the state to change using peek rather that reacquiring
      // the lock over and over.  We do this because our spinlocks are not fair and we want to give
      // the thread responsible for opening/closing the gate a better chance of acquiring the lock
      // so it can complete its task and transition the state to either open or closed.
      State s = peek_state();
      while (s != State::kOpen && s != State::kClosed) {
        arch::Yield();
        s = peek_state();
      }
    }
  }

  // Open the gate and leave the region.  This is intended to be called after a call to |TryLeave|
  // has returned false and the caller has performed any necessary resource management tasks.
  //
  // It is an error to call this method with interrupts enabled.
  void OpenLeave() TA_EXCL(lock_) {
    Guard<SpinLock, NoIrqSave> guard{&lock_};
    DEBUG_ASSERT_MSG(State s = state_.load(); s == State::kOpening && count_ == 0,
                                              "state=%u, count=%u", static_cast<uint32_t>(s),
                                              count_);
    state_.store(State::kOpen);
    count_ += 1;
  }

 private:
  enum class State : uint32_t {
    // Gate is open.
    //
    // Valid transitions from kOpen:
    //   TryEnter:
    //     count > 1:  --> kOpen
    //     count == 1: --> kOpening
    //   TryLeave:
    //     count > 0:  --> kOpen
    //     count == 0: --> kOpening
    kOpen = 0u,

    // Gate is closing.
    //
    // Valid transitions from kClosing:
    //   CloseEnter:
    //     count == 1: --> kClosed
    //   TryLeave:
    //     count == 1: --> try again later
    kClosing,

    // Gate is closed.
    //
    // Valid transitions from kClosed:
    //   TryLeave:
    //     count == 0: --> kOpening
    kClosed,

    // Gate is opening.
    //
    // Valid transitions from kOpening:
    //   OpenLeave:
    //     count == 0: --> kOpen
    //   TryLeave:
    //     count == 0: --> try again later
    kOpening,
  };

  // Returns the state without having to hold the lock.
  State peek_state() const TA_EXCL(lock_) {
    return [this]() TA_NO_THREAD_SAFETY_ANALYSIS { return state_.load(); }();
  }

  DECLARE_SPINLOCK(ControlRegion) lock_;

  // Note, this field is an atomic so that it may be read via |peek_state| without holding the lock.
  TA_GUARDED(lock_)
  RelaxedAtomic<State> state_{State::kOpen};

  // The number of CPUs outside the region.
  //
  // TODO(https://fxbug.dev/450973098): This value serves double duty in that it counts both the
  // number of CPUs outside the region and the number of CPUs that could enter the region.  Revisit
  // and consider tracking them separately so we can better enforce the invariants.  See also code
  // review discussion leading to this comment.
  TA_GUARDED(lock_)
  uint32_t count_{};
};

#endif  // ZIRCON_KERNEL_LIB_CONTROL_REGION_INCLUDE_LIB_CONTROL_REGION_H_
