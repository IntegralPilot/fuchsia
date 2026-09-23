// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifdef _KERNEL

#include <stddef.h>
#include <stdint.h>

#include <new>

#include <lockdep/lockdep.h>

#if WITH_LOCK_DEP || \
    (defined(LOCK_DEP_ENABLED_FEATURE_LEVEL) && LOCK_DEP_ENABLED_FEATURE_LEVEL >= 1)

// Dynamic layout size, alignment, and offset expectations matching the current preprocessor options
#if WITH_LOCK_DEP
constexpr size_t kExpectedLockClassStateSize = sizeof(lockdep::ValidatorLockClassState);
constexpr size_t kExpectedLockClassStateAlign = alignof(lockdep::ValidatorLockClassState);
constexpr size_t kExpectedLockClassRegistrationSize = 1624;
constexpr size_t kExpectedStateStorageOffset = 16;
#else
constexpr size_t kExpectedLockClassStateSize = sizeof(lockdep::MetadataLockClassState);
constexpr size_t kExpectedLockClassStateAlign = alignof(lockdep::MetadataLockClassState);
constexpr size_t kExpectedLockClassRegistrationSize = 24;
constexpr size_t kExpectedStateStorageOffset = 16;
#endif

// Binary-stable FFI representation of Rust LockClassRegistration
struct LockClassRegistration {
  const void* name;
  uint16_t flags;
  alignas(kExpectedLockClassStateAlign) uint8_t state_storage[kExpectedLockClassStateSize];
};

// Static layout verification ensuring exact structural alignment parity with Rust
// LockClassRegistration
static_assert(sizeof(LockClassRegistration) == kExpectedLockClassRegistrationSize,
              "LockClassRegistration size mismatch between C++ and Rust");
static_assert(alignof(LockClassRegistration) == 8,
              "LockClassRegistration alignment mismatch between C++ and Rust");
static_assert(sizeof(LockClassRegistration::state_storage) == kExpectedLockClassStateSize,
              "LockClassRegistration::state_storage size mismatch");

static_assert(offsetof(LockClassRegistration, name) == 0,
              "LockClassRegistration::name offset mismatch");
static_assert(offsetof(LockClassRegistration, flags) == 8,
              "LockClassRegistration::flags offset mismatch");
static_assert(offsetof(LockClassRegistration, state_storage) == kExpectedStateStorageOffset,
              "LockClassRegistration::state_storage offset mismatch");

extern "C" {
extern LockClassRegistration __start_rust_lock_classes[];
extern LockClassRegistration __stop_rust_lock_classes[];
}

// Self-contained static constructor to construct all Rust lock class states at boot time
class RustLockClassRegistrar {
 public:
  RustLockClassRegistrar() {
    for (auto* entry = __start_rust_lock_classes; entry < __stop_rust_lock_classes; ++entry) {
      [[maybe_unused]] const auto& name =
          *reinterpret_cast<const fxt::InternedString*>(entry->name);
#if WITH_LOCK_DEP
      new (entry->state_storage)
          lockdep::ValidatorLockClassState(name, static_cast<lockdep::LockFlags>(entry->flags));
#elif kLockMetadataAvailable
      new (entry->state_storage)
          lockdep::MetadataLockClassState(name, static_cast<lockdep::LockFlags>(entry->flags));
#else
      new (entry->state_storage) lockdep::LockClassState();
#endif
    }
  }
};

static RustLockClassRegistrar registrar;

#if WITH_LOCK_DEP
extern "C" void lockdep_assert_no_locks_held() { lockdep::AssertNoLocksHeld(); }
#endif

#endif  // WITH_LOCK_DEP || feature level >= 1

#endif  // _KERNEL
