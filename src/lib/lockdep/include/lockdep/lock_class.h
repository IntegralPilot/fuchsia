// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_CLASS_H_
#define SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_CLASS_H_

#include <lib/fxt/interned_string.h>
#include <zircon/compiler.h>

#include <type_traits>
#include <utility>

#include <fbl/macros.h>
#include <fbl/type_info.h>
#include <lockdep/common.h>
#include <lockdep/global_reference.h>
#include <lockdep/lock_class_state.h>
#include <lockdep/lock_name_helper.h>
#include <lockdep/lock_traits.h>
#include <lockdep/thread_lock_state.h>

namespace lockdep {

// A fbl helper which can be used to test to see if an object's definition includes a method of the
// form:
//
// ```
// void SetLockClassId(LockClassId id)
// ```
//
// Used by the Lock's state wrappers to inform an underlying lock implementation
// of its class ID via a set method when supported.
//
DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_set_lock_class_id, SetLockClassId,
                                     void (C::*)(LockClassId));

// Looks up a lock traits type using ADL to perform cross-namespace matching.
// This alias works in concert with the macro LOCK_DEP_TRAITS(type, flags) to
// find the defined lock traits for a lock, even when defined in a different
// namespace.
template <typename T>
using LookupLockTraits = decltype(LOCK_DEP_GetLockTraits(static_cast<T*>(nullptr)));

// Base lock traits type. If a type has not been tagged with the
// LOCK_DEP_TRAITS() then this base type provides the default flags for the lock
// type.
template <typename T, typename = void>
struct LockTraits {
  static constexpr LockFlags Flags = LockFlagsNone;
};
// Returns the flags for a lock type when the type is tagged with
// LOCK_DEP_TRAITS(type, flags).
template <typename T>
struct LockTraits<T, std::void_t<LookupLockTraits<T>>> {
  static constexpr LockFlags Flags = LookupLockTraits<T>::Flags;
};

// Forward declarations.
template <typename Class, typename LockType, size_t Index, LockFlags Flags>
class LockClass;

namespace internal {

// Generates the global storage for runtime validation state and/or lock
// metadata, depending on the compile-time configuration.
template <bool Enabled, typename Class, typename LockType, size_t Index, LockFlags Flags>
class LockClassStateStorage {
 public:
  static constexpr LockClassId id() { return lock_class_state_.id(); }

 private:
  // Select whether to generate full validation state or only metadata.
  using LockClassStateType =
      IfLockValidationEnabled<ValidatorLockClassState, MetadataLockClassState>;

  // This static member serves a dual role:
  //  1. It generates storage for the lock class metadata and validation state.
  //  2. The address of this member serves as the unique id for this lock class.
  inline static LockClassStateType lock_class_state_{
      internal::LockNameHelper<Class, Index>::GetInternedString(),
      static_cast<LockFlags>(LockTraits<LockType>::Flags | Flags)};
};

// Empty storage case that does not generate runtime validation or metadata
// storage when both are disabled at compile-time.
template <typename Class, typename LockType, size_t Index, LockFlags Flags>
class LockClassStateStorage<false, Class, LockType, Index, Flags> {
 public:
  static constexpr LockClassId id() { return kInvalidLockClassId; }
};

}  // namespace internal

// Singleton type representing a lock class. Each unique template instantiation
// represents an independent, unique lock class. This type generates global lock
// metadata and runtime validation state structures when these respective
// features are enabled.
template <typename Class, typename LockType, size_t Index, LockFlags Flags>
class LockClass : public internal::LockClassStateStorage<kLockMetadataAvailable, Class, LockType,
                                                         Index, Flags> {};

// Utility to normalize the LockClass type by removing global references from
// the LockType.
template <typename Class, typename LockType, size_t Index, LockFlags Flags>
using NormalizedLockClass = LockClass<Class, RemoveGlobalReference<LockType>, Index, Flags>;

// Base lock wrapper type that provides the essential interface required by
// Guard<LockType, Option> to perform locking and validation. This type wraps
// an instance of LockType that is used to perform the actual synchronization.
// When lock validation is enabled this type also stores the LockClassId for
// the lock class this lock belongs to.
//
// The "lock class" that each lock belongs to is created by each unique
// instantiation of the types LockDep<Class, LockType, Index> or
// SingletonLockDep<Class, LockType, LockFlags> below. These types subclass
// Lock<LockType> to provide type erasure required when virtual accessors are
// used to specify capabilities to Clang static lock analysis.
//
// For example, the lock_ members of LockableType and AnotherLockableType below
// are different types due to how LockDep<> instruments the lock members.
// Both unique LockDep<> instantiations derive from the same Lock<LockType>,
// providing a common capability type (erasing the LockDep<> type) that may be
// used in static lock annotations with the expression "get_lock()".
//
//  struct LockableInterface {
//      virtual ~LockableInterface = 0;
//      virtual Lock<fbl::Mutex>* get_lock() = 0;
//      virtual void Clear() __TA_REQUIRES(get_lock()) = 0;
//  };
//
//  class LockableType : public LockableInterface {
//  public:
//      LockableType() = default;
//      ~LockableType() override = default;
//
//      Lock<fbl::Mutex>* get_lock() override { return &lock_; }
//
//      void Clear() override { count_ = 0; }
//
//      void Increment() {
//          Guard<fbl::Mutex> guard{get_lock()};
//          count_++;
//      }
//
//
//  private:
//      LOCK_DEP_INSTRUMENT(LockableType, fbl::Mutex) lock_;
//      int count_ __TA_GUARDED(get_lock()) {0};
//  };
//
//  class AnotherLockableType : public LockableInterface {
//  public:
//      AnotherLockableType() = default;
//      ~AnotherLockableType() override = default;
//
//      Lock<fbl::Mutex>* get_lock() override { return &lock_; }
//
//      void Clear() override { test_.clear(); }
//
//      void Append(const std::string& string) {
//          Guard<fbl::Mutex> guard{get_lock()};
//          text_.append(string)
//      }
//
//
//  private:
//      LOCK_DEP_INSTRUMENT(AnotherLockableType, fbl::Mutex) lock_;
//      std::string text_ __TA_GUARDED(get_lock()) {};
//  };
//
template <typename LockType>
class __TA_CAPABILITY("mutex") Lock {
 public:
  Lock(Lock&&) = delete;
  Lock(const Lock&) = delete;
  Lock& operator=(Lock&&) = delete;
  Lock& operator=(const Lock&) = delete;

  ~Lock() = default;

  // Provides direct access to the underlying lock. Care should be taken when
  // manipulating the underlying lock. Incorrect manipulation could confuse
  // the validator, trigger lock assertions, and/or deadlock.
  LockType& lock() __TA_RETURN_CAPABILITY(*this) { return state_.lock_; }
  const LockType& lock() const __TA_RETURN_CAPABILITY(*this) { return state_.lock_; }

  // Returns the capability of the underlying lock. This is expected by Guard
  // as an additional static assertion target.
  LockType& capability() __TA_RETURN_CAPABILITY(state_.lock_) { return state_.lock_; }
  const LockType& capability() const __TA_RETURN_CAPABILITY(state_.lock_) { return state_.lock_; }

  // Returns the LockClassId of the lock class this lock belongs to.
  constexpr LockClassId id() const { return state_.id(); }

 protected:
  // Initializes the Lock instance with the given LockClassId and passes any
  // additional arguments to the underlying lock constructor.
  template <typename... Args>
  explicit constexpr Lock(LockClassId id, Args&&... args)
      : state_{id, std::forward<Args>(args)...} {}

 private:
  template <typename, typename, typename>
  friend class Guard;
  template <size_t, typename, typename>
  friend class GuardMultiple;

  // State type when lock validation or metadata is enabled.
  struct AvailableState {
    template <typename... Args>
    explicit constexpr AvailableState(LockClassId id, Args&&... args)
        : id_{id}, lock_(std::forward<Args>(args)...) {
      if constexpr (has_set_lock_class_id_v<LockType>) {
        lock_.SetLockClassId(id);
      }
    }

    constexpr LockClassId id() const { return id_; }

    LockClassId id_;
    LockType lock_;
  };

  // State type when validation and metadata is disabled.
  struct UnavailableState {
    template <typename... Args>
    explicit constexpr UnavailableState(LockClassId id, Args&&... args)
        : lock_(std::forward<Args>(args)...) {
      if constexpr (has_set_lock_class_id_v<LockType>) {
        lock_.SetLockClassId(id);
      }
    }

    constexpr LockClassId id() const { return kInvalidLockClassId; }

    [[no_unique_address]] LockType lock_;
  };

  // Selects which state type to use based on whether metadata or validation is
  // enabled.
  using State = IfLockMetadataAvailable<AvailableState, UnavailableState>;

  // State instance holding the underlying lock and, when validation is enabled,
  // the lock class id.
  [[no_unique_address]] State state_;
};

// Specialization of Lock<LockType> that wraps a static/global raw lock. This
// type permits creating a tracked alias of a raw lock of type LockType that
// has static storage duration, with either external or internal linkage.
//
// This type supports transitioning from C-compatible APIs to full C++.
template <typename LockType, LockType& Reference>
class __TA_CAPABILITY("mutex") Lock<GlobalReference<LockType, Reference>> {
 public:
  Lock(Lock&&) = delete;
  Lock(const Lock&) = delete;
  Lock& operator=(Lock&&) = delete;
  Lock& operator=(const Lock&) = delete;

  ~Lock() = default;

  // Provides direct access to the underlying lock. Care should be taken when
  // manipulating the underlying lock. Incorrect manipulation could confuse
  // the validator, trigger lock assertions, and/or deadlock.
  LockType& lock() __TA_RETURN_CAPABILITY(*this) { return Reference; }
  const LockType& lock() const __TA_RETURN_CAPABILITY(*this) { return Reference; }

  // Returns the LockClassId of the lock class this lock belongs to.
  LockClassId id() const { return id_.value(); }

 protected:
  explicit constexpr Lock(LockClassId id) : id_{id} {
    // Do not interact with the lock at this point in time (not even to try to
    // set its lock class ID).  This specialized version of |Lock| does not
    // encapsulate the actual lock, it only holds a reference to it.
    // Additionally, this wrapper is used in situations where the lock in
    // question is almost certainly a global lock.
    //
    // So, we have a global ctor race.  We do not know who is going to be
    // constructed first, the lock we hold a reference to, or the wrapper (aka,
    // |this|).  Attempting to interact with the lock via reference at this
    // point runs the risk of interacting with an object before it has been
    // constructed, which is clearly no good.
  }

 private:
  template <typename, typename, typename>
  friend class Guard;
  template <size_t, typename, typename>
  friend class GuardMultiple;

  struct Value {
    LockClassId value_;
    LockClassId value() const { return value_; }
  };
  struct Disabled {
    constexpr Disabled(LockClassId) {}
    LockClassId constexpr value() const { return kInvalidLockClassId; }
  };

  using IdValue = IfLockMetadataAvailable<Value, Disabled>;

  // Stores the lock class id of this lock when validation is enabled.
  IdValue id_;
};

// Lock wrapper class that implements lock dependency checks. The template
// argument |Class| should be a type that uniquely defines the class, such as
// the type of the containing scope. The template argument |LockType| is the
// type of the lock to wrap. The template argument |Index| may be used to
// differentiate lock classes between multiple locks within the same scope.
//
// For example:
//
//  struct MyType {
//      LockDep<MyType, Mutex, 0> lock_a;
//      LockDep<MyType, Mutex, 1> lock_b;
//      // ...
//  };
//
template <typename Class, typename LockType_, size_t Index = 0, LockFlags Flags = LockFlagsNone>
class LockDep : public Lock<LockType_> {
 public:
  // Expose the wrapped lock type to user via an alias.
  using LockType = LockType_;

  // Alias that may be used by subclasses to simplify constructor
  // expressions.
  using Base = LockDep;

  // Alias of the lock class that this wrapper represents.
  using LockClass = NormalizedLockClass<Class, LockType, Index, Flags>;

  // Constructor that initializes the underlying lock with the additional
  // arguments.
  template <typename... Args>
  constexpr LockDep(Args&&... args)
      : Lock<LockType>(LockClass::id(), std::forward<Args>(args)...) {}
};

// Singleton version of the lock wrapper above. This type is appropriate for
// global locks. This type is used by the macros LOCK_DEP_SINGLETON_LOCK and
// LOCK_DEP_SINGLETON_LOCK_WRAPPER to define instrumented global locks.
template <typename Class, typename LockType_, LockFlags Flags = LockFlagsNone>
class SingletonLockDep : public Lock<LockType_> {
 public:
  // Expose the wrapped lock type to user via an alias.
  using LockType = LockType_;

  // Alias of the lock class that this wrapper represents.
  using LockClass = NormalizedLockClass<Class, LockType, 0, Flags>;

  // Returns a pointer to the singleton object.
  static Class* Get() { return &global_lock_; }

 protected:
  // Initializes the base Lock<LockType> with lock class id for this lock. The
  // additional arguments are pass to the underlying lock.
  template <typename... Args>
  constexpr SingletonLockDep(Args&&... args)
      : Lock<LockType>(LockClass::id(), std::forward<Args>(args)...) {}

 private:
  inline static Class global_lock_;
};

}  // namespace lockdep

#endif  // SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_CLASS_H_
