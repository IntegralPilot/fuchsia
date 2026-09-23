// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_CAPABILITY_TOKEN_H_
#define LIB_CONCURRENT_CAPABILITY_TOKEN_H_

#include <zircon/compiler.h>

#include <type_traits>

namespace concurrent {

// A capability token type to enable the use of Clang thread annotations to check program invariants
// beyond simple locking requirements.
//
// A constexpr capability instance may be defined in a header in global, or preferably namespace
// scope, to be shared among multiple translation units. It may also be defined locally with a
// single translation unit if broader use is unnecessary.
//
// Example:
//
// #ifndef TRANSACTION_H_
// #define TRANSACTION_H_
//
// #include <lib/concurrent/capability_token.h>
//
// namespace acme {
//
// constexpr concurrent::CapabilityToken transaction_active;
//
// class Transaction {
//  public:
//   Transaction() __TA_ACQUIRE(transaction_active) {
//     ZX_DEBUG_ASSERT(Active() == nullptr);
//     SetActive(this);
//   }
//   ~Transaction() __TA_RELEASE(transaction_active) {
//     ZX_DEBUG_ASSERT(Active() == this);
//     SetActive(nullptr);
//    }
//
//    // Returns the active transaction or nullptr.
//    static Transaction* Active() {
//      return transaction_tl_;
//    }
//
//    // Operation that requires an active transaction. May be called from a scope where the
//    // Transaction instance is only conveniently accessible via Transaction::Active().
//    // Transitive dependencies on this operation must be annotated to require the same
//    // capability, aiding callers in determining that the API requires an active transaction
//    // as a pre-condition.
//    void Operation() __TA_REQUIRES(transaction_active);
//
//  private:
//    void SetActive(Transaction* transaction) {
//      transaction_tl_ = transaction;
//    }
//
//    inline static thread_local Transaction* transaction_tl_;
// };
//
// class UseTransaction {
//  public:
//    void DoSomething() __TA_REQUIRES(transaction_active) {
//      // Perform operations that depend on an active transaction ...
//      // Runtime assertion that Active() != nullptr is unnecessary.
//      Transaction::Active()->Operation();
//    }
// };
//
// }  // namespace acme
//
// #endif  // TRANSACTION_H_
//

// A capability token that may be acquired/released by the given Domain type. The Domain type is
// responsible for performing the actions or checks required to actually acquire/release the
// capability.
//
// Example:
//
// class Foo; // Forward declaration.
//
// constexpr concurrent::CapabilityToken<Foo> foo_token;
//
// class Foo {
//  public:
//   void Acquire() __TA_ACQUIRE(foo_token) {
//     // Operations necessary to to acquire the token...
//     foo_token.Acquire();
//   }
//
//   void Release() __TA_RELEASE(foo_token) {
//     // Operations necessary to to release the token...
//     foo_token.Release();
//   }
// };
//
template <typename Domain = void>
class __TA_CAPABILITY("token") CapabilityToken {
 public:
  CapabilityToken() = default;
  ~CapabilityToken() = default;

  CapabilityToken(const CapabilityToken&) = delete;
  CapabilityToken& operator=(const CapabilityToken&) = delete;
  CapabilityToken(CapabilityToken&&) = delete;
  CapabilityToken& operator=(CapabilityToken&&) = delete;

  // A tag type passed to Domain::AssertCapabilityHeld, if provided by the Domain type, to avoid
  // potential collisions with unrelated methods of the same name.
  enum TagType { Tag };

  // Asserts that the capability is held, if supported by the Domain type.
  //
  // May only be called if the Domain type implements a method with the following signature to
  // perform the actual runtime assertion logic:
  //
  // static void Domain::AssertCapabilityHeld(CapabilityToken<Domain>::TagType)
  //
  constexpr void AssertHeld() const __TA_ASSERT() {
    constexpr bool assert_implemented = HasAssertCapabilityHeldMethod<Domain>::value;
    static_assert(
        assert_implemented,
        "CapabilityToken<Domain>::AssertHeld cannot be called becasue Domain does not implement an "
        "AssertCapabilityHeld method to perform the assert operation.");

    if constexpr (assert_implemented) {
      Domain::AssertCapabilityHeld(Tag);
    }
  }

 private:
  // Allow the Domain type to call Acquire/Release.
  friend Domain;

  // Private acquire/release may be called only from within Domain.
  constexpr void Acquire() const __TA_ACQUIRE() {}
  constexpr void Release() const __TA_RELEASE() {}
  constexpr void AcquireShared() const __TA_ACQUIRE_SHARED() {}
  constexpr void ReleaseShared() const __TA_RELEASE_SHARED() {}

  // Detects whether type T has a method with the following signature:
  //
  // static void T::AssertCapabilityHeld(CapabilityToken<Domain>::TagType)
  //
  template <typename T, typename = void>
  struct HasAssertCapabilityHeldMethod : std::false_type {};

  template <typename T>
  struct HasAssertCapabilityHeldMethod<T, std::void_t<decltype(T::AssertCapabilityHeld)>>
      : std::is_invocable_r<void, decltype(T::AssertCapabilityHeld), TagType> {};
};

// A capability token similar to the type above but without the domain-restricted access to
// acquire/release the capability.
template <>
class __TA_CAPABILITY("token") CapabilityToken<void> {
 public:
  CapabilityToken() = default;
  ~CapabilityToken() = default;

  CapabilityToken(const CapabilityToken&) = delete;
  CapabilityToken& operator=(const CapabilityToken&) = delete;
  CapabilityToken(CapabilityToken&&) = delete;
  CapabilityToken& operator=(CapabilityToken&&) = delete;

  // Public acquire/release may be called from any context.
  constexpr void Acquire() const __TA_ACQUIRE() {}
  constexpr void Release() const __TA_RELEASE() {}
  constexpr void AcquireShared() const __TA_ACQUIRE_SHARED() {}
  constexpr void ReleaseShared() const __TA_RELEASE_SHARED() {}

 private:
  // Intentionally not implemented. Use cases must provide their own external assert implementation
  // that performs the required checks when asserts are enabled.
  void AssertHeld() __TA_ASSERT();
};

}  // namespace concurrent

#endif  // LIB_CONCURRENT_CAPABILITY_TOKEN_H_
