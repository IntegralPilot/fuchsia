// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FBL_REF_COUNTED_INTERNAL_H_
#define FBL_REF_COUNTED_INTERNAL_H_

#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <atomic>

namespace fbl {
namespace internal {

// Adoption validation will help to catch:
// - Double-adoptions
// - AddRef/Release without adopting first
// - Re-wrapping raw pointers to destroyed objects
//
// It also provides some limited defense against
// - Wrapping bad pointers
template <bool EnableAdoptionValidator>
class RefCountedBase {
 protected:
  constexpr RefCountedBase() : ref_count_(kPreAdoptSentinel) {}

  ~RefCountedBase() {
    if constexpr (EnableAdoptionValidator) {
      // Reset the ref-count back to the pre-adopt sentinel value so that we
      // have the best chance of catching a use-after-free situation, even if
      // we have a messed up mix of debug/release translation units being
      // linked together.
      ref_count_.store(kPreAdoptSentinel, std::memory_order_release);
    }
  }

  void AddRef() const {
    const int32_t rc = ref_count_.fetch_add(1, std::memory_order_relaxed);

    // This assertion will fire if either of the following occur.
    //
    // 1) someone calls AddRef() before the object has been properly
    // Adopted.
    //
    // 2) someone calls AddRef() on a ref-counted object that has
    // reached ref_count_ == 0 but has not been destroyed yet. This
    // could happen by manually calling AddRef(), or re-wrapping such a
    // pointer with RefPtr<T>(T*) (which calls AddRef()).
    //
    // Note: leave the ASSERT on in all builds.  The constant
    // EnableAdoptionValidator check above should cause this code path to be
    // pruned in release builds, but leaving this as an always on ASSERT
    // will mean that the tests continue to function even when built as
    // release.
    if constexpr (EnableAdoptionValidator) {
      ZX_ASSERT_MSG(rc >= 1, "count %d(0x%08x) < 1\n", rc, static_cast<uint32_t>(rc));
    }
  }

  // Returns true if the object should self-delete.
  //
  // A call to Release() that returns true must synchronize-with the previous
  // (in the modification order of ref_count_) call to Release().  See below for
  // details.
  bool Release() const __WARN_UNUSED_RESULT {
    //
    // To ensure that all accesses to a ref-counted object happen-before the
    // last RefPtr to the object is released, we must ensure that a call to
    // Release() that drops the ref_count_ to zero, synchronizes-with the
    // previous call that dropped the count to one.
    //
    // Both the use of std::memory_order_release when decrementing ref_count_,
    // and the use of std::memory_order_acquire on the std::atomic_thread_fence
    // below are critical to ensuring this synchronizes-with behavior.  To
    // understand why, consider the following example...
    //
    //
    // Example 1: correct if RefPtr provides synchronization
    //
    // class Value : public fbl::RefCounted<Value> {
    //  public:
    //   explicit Value(int v) : v_(v) {}
    //   int get() const { return v_; }
    //  private:
    //   const int v_;
    // };
    //
    // void Example(int v) {
    //   fbl::RefPtr<Value> p1 = fbl::AdoptRef(new Value(v));
    //   fbl::RefPtr<Value> p2 = p1;
    //
    //   int v1;
    //   std::thread t1([&p1, &v1]() {
    //     v1 = p1->get();
    //     p1.reset();
    //   });
    //
    //   int v2;
    //   std::thread t2([&p2, &v2]() {
    //     v2 = p2->get();
    //     p2.reset();
    //   });
    //
    //   t1.join();
    //   t2.join();
    //
    //   printf("sum is %d\n", v1 + v2);
    // }
    //
    //
    // In the example above, we have two threads, t1 and t2.  Each thread
    // accesses the shared instance using its own RefPtr, makes an observation
    // via get(), then resets its pointer.  Under the hood, reset() calls
    // Release().  When Release() returns true, reset() will destroy the object.
    //
    // For this example to be correct, RefPtr must ensure all accesses to
    // the ref-counted object happen-before the last reference to the object is
    // released, that is, before Release() returns true.
    //
    // What happens if Release() does not provide synchronization?  Let's take a
    // look at a similar, but incorrect example.  If we strip it down, "inline"
    // RefPtr, and remove the synchronization we get something like...
    //
    //
    // // Example 2: incorrect, no synchronization
    //
    // bool Release(std::atomic<int>& rc) {
    //   return rc.fetch_sub(1, std::memory_order_relaxed) == 1;
    // }
    //
    // void Example(int v) {
    //   const int* p = new int(v);
    //   std::atomic<int> ref_count = 2;
    //
    //   int v1;
    //   std::thread t1([&p, &v1]() {
    //     v1 = *p;
    //     if (Release(ref_count)) {
    //       delete p;
    //     }
    //   });
    //
    //   int v2;
    //   std::thread t2([&p, &v2]() {
    //     v2 = *p;
    //     if (Release(ref_count)) {
    //       delete p;
    //     }
    //   });
    //
    //   t1.join();
    //   t2.join();
    //
    //   printf("sum is %d\n", v1 + v2);
    // }
    //
    //
    // The above example has a bug because there is nothing preventing the
    // compiler or hardware from reordering the thread's dereference of p with
    // its decrement of ref_count.  In other words, this,
    //
    //     v1 = *p;
    //     if (Release(ref_count)) {
    //       delete p;
    //     }
    //
    // could be transformed into this,
    //
    //     bool should_delete = Release(ref_count);
    //     v1 = *p;
    //     if (should_delete) {
    //       delete p;
    //     }
    //
    // To prevent the possibility of a reordering-induced use-after-free or a
    // destructor racing with "previous" object accesses, we to must make
    // Release() synchronize-with Release().  There are multiple ways to do it.
    //
    // **decrement with acq_rel** - This approach uses std::memory_order_acq_rel
    // when decrementing ref_count_.  By using std::memory_order_acq_rel, every
    // decrement operation acts as both an acquire operation and a release
    // operation, so a decrement A, that observes the value written by a
    // decrement B, synchronizes-with B.
    //
    // **decrement with release plus an acquire fence** - Using
    // std::memory_order_acq_rel for the decrement operation actually provides
    // more synchronization than is strictly necessary.  We only need to
    // synchronize the Release calls that decrement the ref_count_ to one or
    // zero.  Instead of using std::memory_order_acq_rel, we can use
    // std::memory_order_release and then in the case where we did, in fact,
    // decrement the ref_count_ to zero, issue an std::atomic_thread_fence with
    // std::memory_order_acquire (atomic-fence synchronization).
    //
    // It should be noted that on some machines one of the two approaches may
    // perform better than the other.  We have not analyzed the performance and
    // may wish to revisit our selection.
    //
    const int32_t rc = ref_count_.fetch_sub(1, std::memory_order_release);

    // This assertion will fire if someone manually calls Release()
    // on a ref-counted object too many times, or if Release is called
    // before an object has been Adopted.
    //
    // Note: leave the ASSERT on in all builds.  The constant
    // EnableAdoptionValidator check above should cause this code path to be
    // pruned in release builds, but leaving this as an always on ASSERT
    // will mean that the tests continue to function even when built as
    // release.
    if constexpr (EnableAdoptionValidator) {
      ZX_ASSERT_MSG(rc >= 1, "count %d(0x%08x) < 1\n", rc, static_cast<uint32_t>(rc));
    }

    if (rc == 1) {
      std::atomic_thread_fence(std::memory_order_acquire);
      return true;
    }

    return false;
  }

  bool IsLastReference() const __WARN_UNUSED_RESULT {
    return ref_count_.load(std::memory_order_seq_cst) == 1;
  }

  void Adopt() const {
    if constexpr (EnableAdoptionValidator) {
      int32_t expected = kPreAdoptSentinel;
      bool res = ref_count_.compare_exchange_strong(expected, 1, std::memory_order_acq_rel,
                                                    std::memory_order_acquire);
      // Note: leave the ASSERT on in all builds.  The constant
      // EnableAdoptionValidator check above should cause this code path
      // to be pruned in release builds, but leaving this as an always on
      // ASSERT will mean that the tests continue to function even when
      // built as release.
      ZX_ASSERT_MSG(res, "count(0x%08x) != sentinel(0x%08x)\n", static_cast<uint32_t>(expected),
                    static_cast<uint32_t>(kPreAdoptSentinel));
    } else {
      ref_count_.store(1, std::memory_order_release);
    }
  }

  // Current ref count. Only to be used for debugging purposes.
  int ref_count_debug() const { return ref_count_.load(std::memory_order_relaxed); }

  // Note:
  //
  // The PreAdoptSentinel value is chosen specifically to be negative when
  // stored as an int32_t, and as far away from becoming positive (via either
  // addition or subtraction) as possible.  These properties allow us to
  // combine the debug-build adopt sanity checks and the lifecycle sanity
  // checks into a single debug assert.
  //
  // If a user creates an object, but never adopts it, they would need to
  // perform 0x4000000 (about 1 billion) unchecked AddRef or Release
  // operations before making the internal ref_count become positive again.
  // At this point, even a checked AddRef or Release operation would fail to
  // detect the bad state of the system fails to detect the problem.
  //
  static constexpr int32_t kPreAdoptSentinel = static_cast<int32_t>(0xC0000000);
  mutable std::atomic_int32_t ref_count_;
};

}  // namespace internal
}  // namespace fbl

#endif  // FBL_REF_COUNTED_INTERNAL_H_
