// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_CONSTANTS_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_CONSTANTS_H_

#include <stddef.h>
#include <zircon/types.h>

constexpr uint32_t kVmCachePolicyMask = ZX_CACHE_POLICY_MASK;

// Size and alignment constants for Rust VM states stored in C++ OpaqueStorage.
// These values must match the exact size and alignment of their corresponding Rust structs,
// which is enforced by static_asserts in both Rust and C++.

// Size, alignment, and offset for VmObjectPhysicalState.
//
// 1. The state size is conditional because VmObjectPhysicalState contains a ksync::KMutex
//    which wraps a lockdep-instrumented raw lock. When WITH_LOCK_DEP is enabled, the lock
//    contains a lock class ID pointer (class_id_), which increases the state size by 8 bytes.
//    Additionally, lock name tracing (when enabled via GN parameters) can increase the lock
//    size by another 8 bytes. To support all build variants robustly, we use sizing limits
//    (80 with lockdep, 72 without) that accommodate lock name tracing.
//
// 2. The offset is conditional because VmObjectPhysical inherits from VmObject, which contains
//    an fbl::Name member (name_). fbl::Name has a lockdep-instrumented lock member (lock_).
//    - When lockdep is disabled, lock_ is empty and uses [[no_unique_address]], occupying
//      0 bytes. This makes fbl::Name 32 bytes, placing child_observer_ at offset 120, and resulting
//      in an opaque_storage_ offset of 136 bytes.
//    - When lockdep is enabled, lock_ is 16 bytes (aligned to 8), making fbl::Name 48 bytes,
//      placing child_observer_ at offset 136, and resulting in an opaque_storage_ offset of
//      152 bytes.
#if WITH_LOCK_DEP
constexpr size_t kVmObjectPhysicalStateSize = 80;
#else
constexpr size_t kVmObjectPhysicalStateSize = 72;
#endif
constexpr size_t kVmObjectPhysicalStateAlign = 8;
#if WITH_LOCK_DEP
constexpr size_t kVmObjectPhysicalStateOffset = 152;
#else
constexpr size_t kVmObjectPhysicalStateOffset = 136;
#endif

// VM Page List Constants
constexpr uint32_t kPmmNodeIndexZeroBits = 3;
constexpr uint32_t kVmPageListTypeBits = 3;
constexpr uint32_t kVmPageListPageType = 0b000;
constexpr uint32_t kVmPageListReferenceType = 0b010;
constexpr uint32_t kVmPageListZeroMarkerType = 0b001;
constexpr uint32_t kVmPageListIntervalType = 0b011;
constexpr uint32_t kVmPageListParentContentType = 0b100;

// Interval sentinel limits
constexpr uint32_t kVmPageListIntervalSentinelBits = 2;
constexpr uint32_t kVmPageListIntervalTypeBits = 2;
constexpr uint32_t kVmPageListIntervalBits =
    kVmPageListTypeBits + kVmPageListIntervalSentinelBits + kVmPageListIntervalTypeBits;

// Fanout of a single VmPageListNode (number of page slots in a node).
constexpr size_t kVmPageListFanOut = 16;

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_CONSTANTS_H_
