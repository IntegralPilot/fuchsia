// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZIRCON_INTERNAL_KTRACE_H_
#define LIB_ZIRCON_INTERNAL_KTRACE_H_

// clang-format off

// Category bits.
enum {
  KTRACE_GRP_META_BIT = 0,
  KTRACE_GRP_MEMORY_BIT,
  KTRACE_GRP_SCHEDULER_BIT,
  KTRACE_GRP_CONTENTION_BIT,
  KTRACE_GRP_IPC_BIT,
  KTRACE_GRP_IRQ_BIT,
  KTRACE_GRP_PROBE_BIT,
  KTRACE_GRP_ARCH_BIT,
  KTRACE_GRP_SYSCALL_BIT,
  KTRACE_GRP_VM_BIT,
  KTRACE_GRP_RESTRICTED_BIT,
  KTRACE_GRP_POWER_BIT,
  KTRACE_GRP_OOM_BIT,

  // Keep last.
  KTRACE_GRP_NEXT_UNUSED_BIT,
};

// Filter Groups
#define KTRACE_GRP_ALL            ((1u << KTRACE_GRP_NEXT_UNUSED_BIT) - 1)
#define KTRACE_GRP_META           (1u << KTRACE_GRP_META_BIT)
#define KTRACE_GRP_MEMORY         (1u << KTRACE_GRP_MEMORY_BIT)
#define KTRACE_GRP_SCHEDULER      (1u << KTRACE_GRP_SCHEDULER_BIT)
#define KTRACE_GRP_CONTENTION     (1u << KTRACE_GRP_CONTENTION_BIT)
#define KTRACE_GRP_IPC            (1u << KTRACE_GRP_IPC_BIT)
#define KTRACE_GRP_IRQ            (1u << KTRACE_GRP_IRQ_BIT)
#define KTRACE_GRP_PROBE          (1u << KTRACE_GRP_PROBE_BIT)
#define KTRACE_GRP_ARCH           (1u << KTRACE_GRP_ARCH_BIT)
#define KTRACE_GRP_SYSCALL        (1u << KTRACE_GRP_SYSCALL_BIT)
#define KTRACE_GRP_VM             (1u << KTRACE_GRP_VM_BIT)
#define KTRACE_GRP_RESTRICTED     (1u << KTRACE_GRP_RESTRICTED_BIT)
#define KTRACE_GRP_POWER          (1u << KTRACE_GRP_POWER_BIT)
#define KTRACE_GRP_OOM            (1u << KTRACE_GRP_OOM_BIT)

// Actions for ktrace control
#define KTRACE_ACTION_START          1 // options = grpmask, 0 = all
#define KTRACE_ACTION_STOP           2 // options ignored
#define KTRACE_ACTION_REWIND         3 // options ignored
#define KTRACE_ACTION_START_CIRCULAR 5 // options = grpmask, 0 = all

#endif  // LIB_ZIRCON_INTERNAL_KTRACE_H_
