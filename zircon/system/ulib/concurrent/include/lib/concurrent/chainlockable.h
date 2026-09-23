// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_CHAINLOCKABLE_H_
#define LIB_CONCURRENT_CHAINLOCKABLE_H_

#include <lib/concurrent/chainlock.h>

namespace concurrent {

// ChainLockable is a utility base class that solves common problems for types that use chain locks
// to protect some or all of their invariants. One problem this utility addresses is difficulty
// referring to chain lock capabilities in static annotations involving cross-referencing types,
// which are common in graphs with heterogeneous node types.
//
// The following example illustrates cross-referencing types, where TypeA needs to refer to the
// chain lock capability of TypeB while it is still an incomplete type.
//
//
// class TypeB;
// class TypeA : public ChainLockable {
//  public:
//   explicit TypeA(int value) : value_{value} {}
//
//   // ChainLockable::GetLock() may be used to get the lock capability for incomplete types that
//   // are eventually defined to derive from ChainLockable.
//   bool compare(const TypeB& other) const __TA_REQUIRES(get_lock(),
//                                                        ChainLockable::GetLock(other));
//
//  private:
//   friend class TypeB;
//   __TA_GUARDED(get_lock()) int value_;
// };
//
// class TypeB : public ChainLockable {
//  public:
//   explicit TypeB(int value) : value_{value} {}
//
//   bool compare(const TypeA& other) const __TA_REQUIRES(get_lock(), other.get_lock()) {
//     return value_ == other.value_;
//   }
//
//  private:
//   friend class TypeA;
//   __TA_GUARDED(get_lock()) int value_;
// };
//
// // Must be defined here since it depends on the complete definition of TypeB.
// inline bool TypeA::compare(const TypeB& other) const { return value_ == other.value_; }
//
//
// Possible future extensions of ChainLockable include host-tested utility methods to traverse
// homogeneous or heterogeneous graphs, locking or unlocking chain locks with user defined per-node
// operations and stop point behavior.
//

template <typename Transaction>
class __TA_CAPABILITY("lockable") ChainLockable {
 public:
  ChainLockable() = default;
  ~ChainLockable() = default;

  ChainLock<Transaction>& get_lock() const __TA_RETURN_CAPABILITY(this) { return lock_; }

  template <typename T>
  static ChainLock<Transaction>& GetLock(const T& lockable) __TA_RETURN_CAPABILITY(lockable) {
    static_assert(std::is_same_v<ChainLockable, T>);
    return static_cast<const ChainLockable&>(lockable).get_lock();
  }

 protected:
  mutable ChainLock<Transaction> lock_;
};

}  // namespace concurrent

#endif  // LIB_CONCURRENT_CHAINLOCKABLE_H_
