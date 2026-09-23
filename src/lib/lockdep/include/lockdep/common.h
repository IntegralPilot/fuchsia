// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//
// Common definitions for the lockdep library.
//

#ifndef SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_COMMON_H_
#define SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_COMMON_H_

#include <stddef.h>
#include <stdint.h>

#include <type_traits>

#include <lockdep/runtime_api.h>

namespace lockdep {

// Configures the maximum number of dependencies each lock class may have. A
// system may override the default by globally defining this name to the desired
// value. This value is automatically converted to the next suitable prime.
#ifndef LOCK_DEP_MAX_DEPENDENCIES
#define LOCK_DEP_MAX_DEPENDENCIES 97
#endif

// Configures the level of feature support in lockdep:
// * 0: All runtime features disabled (default).
// * 1: Lock class metadata enabled.
// * 2: Lock validation enabled.
#ifndef LOCK_DEP_ENABLED_FEATURE_LEVEL
#define LOCK_DEP_ENABLED_FEATURE_LEVEL 0
#endif

#define LOCK_DEP_STRINGIFY(x) LOCK_DEP_STRINGIFY2(x)
#define LOCK_DEP_STRINGIFY2(x) #x

// Assert the enabled feature level is valid.
static_assert(
    static_cast<int>(LOCK_DEP_ENABLED_FEATURE_LEVEL) >= 0 &&
        static_cast<int>(LOCK_DEP_ENABLED_FEATURE_LEVEL) <= 2,
    "Invalid value for LOCK_DEP_FEATURE_LEVEL: " LOCK_DEP_STRINGIFY(LOCK_DEP_FEATURE_LEVEL));

// Forward declaration. Most code refers to this type through the quasi-opaque
// type LockClassId.
class LockClassState;

// Id type used to identify each lock class.
using LockClassId = const LockClassState*;

// A sentinel value indicating an empty slot in lock tracking data structures.
constexpr LockClassId kInvalidLockClassId = nullptr;

namespace internal {

// Returns a prime number that reasonably accommodates a hash table of the given
// number of entries. Each number is selected to be slightly less than twice the
// previous and as far as possible from the nearest two powers of two.
constexpr size_t NextPrime(size_t n) {
  if (n < (1 << 4))
    return 23;
  else if (n < (1 << 5))
    return 53;
  else if (n < (1 << 6))
    return 97;
  else if (n < (1 << 7))
    return 193;
  else if (n < (1 << 8))
    return 389;
  else if (n < (1 << 9))
    return 769;
  else if (n < (1 << 10))
    return 1543;
  else
    return 0;  // The input exceeds the size of this prime table.
}

}  // namespace internal

// The maximum number of dependencies each lock class may have. This is the
// maximum branching factor of the directed graph of locks managed by this
// lock dependency algorithm. The value is a prime number selected to optimize
// the hash map in LockDependencySet.
constexpr size_t kMaxLockDependencies = internal::NextPrime(LOCK_DEP_MAX_DEPENDENCIES);

// Check to make sure that the requested max does not exceed the prime table.
static_assert(kMaxLockDependencies != 0, "LOCK_DEP_MAX_DEPENDENCIES too large!");

// Enumeration of supported feature levels.
enum class FeatureLevel : uint8_t {
  Disabled = 0,
  MetadataOnly = 1,
  Validation = 2,
};

// The enabled feature level.
constexpr FeatureLevel kEnabledFeatureLevel =
    static_cast<FeatureLevel>(LOCK_DEP_ENABLED_FEATURE_LEVEL);

// Whether or not lock metadata is enabled when validation is disabled.
constexpr bool kLockMetadataOnlyEnabled = kEnabledFeatureLevel == FeatureLevel::MetadataOnly;

// Whether or not lock validation is globally enabled.
constexpr bool kLockValidationEnabled = kEnabledFeatureLevel == FeatureLevel::Validation;

// Whether or not lock metadata is available.
constexpr bool kLockMetadataAvailable = kLockValidationEnabled || kLockMetadataOnlyEnabled;

// Utility template alias to simplify selecting different types based whether
// lock validation is enabled or disabled.
template <typename EnabledType, typename DisabledType>
using IfLockValidationEnabled =
    std::conditional_t<kLockValidationEnabled, EnabledType, DisabledType>;

// Utility template alias to simplify selecting different types based on whether
// lock metadata is available.
template <typename AvailableType, typename UnavailableType>
using IfLockMetadataAvailable =
    std::conditional_t<kLockMetadataAvailable, AvailableType, UnavailableType>;

// Result type that represents whether a lock attempt was successful, or if not
// which check failed.
enum class LockResult : uint8_t {
  Success,
  AlreadyAcquired,
  OutOfOrder,
  InvalidNesting,
  InvalidIrqSafety,
  Reentrance,
  ShouldNotHold,
  AcquireAfterLeaf,

  // Non-fatal error that indicates the dependency hash set for a particular
  // lock class is full. Consider increasing the size of the lock dependency
  // sets if this error is reported.
  MaxLockDependencies,

  // Internal error value used to differentiate between dependency set updates
  // that add a new edge and those that do not. Only new edges trigger loop
  // detection.
  DependencyExists,
};

// Returns a string representation of the given LockResult.
inline const char* ToString(LockResult result) {
  switch (result) {
    case LockResult::Success:
      return "Success";
    case LockResult::AlreadyAcquired:
      return "Already Acquired";
    case LockResult::OutOfOrder:
      return "Out Of Order";
    case LockResult::InvalidNesting:
      return "Invalid Nesting";
    case LockResult::InvalidIrqSafety:
      return "Invalid Irq Safety";
    case LockResult::Reentrance:
      return "Reentrance";
    case LockResult::ShouldNotHold:
      return "Should Not Hold";
    case LockResult::AcquireAfterLeaf:
      return "Acquire After Leaf";
    case LockResult::MaxLockDependencies:
      return "Max Lock Dependencies";
    case LockResult::DependencyExists:
      return "Dependency Exists";
    default:
      return "Unknown";
  }
}

}  // namespace lockdep

#endif  // SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_COMMON_H_
