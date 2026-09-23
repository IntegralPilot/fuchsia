// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const KTRACE_GRP_META_BIT: u32 = 0;
pub const KTRACE_GRP_MEMORY_BIT: u32 = 1;
pub const KTRACE_GRP_SCHEDULER_BIT: u32 = 2;
pub const KTRACE_GRP_CONTENTION_BIT: u32 = 3;
pub const KTRACE_GRP_IPC_BIT: u32 = 4;
pub const KTRACE_GRP_IRQ_BIT: u32 = 5;
pub const KTRACE_GRP_PROBE_BIT: u32 = 6;
pub const KTRACE_GRP_ARCH_BIT: u32 = 7;
pub const KTRACE_GRP_SYSCALL_BIT: u32 = 8;
pub const KTRACE_GRP_VM_BIT: u32 = 9;
pub const KTRACE_GRP_RESTRICTED_BIT: u32 = 10;
pub const KTRACE_GRP_POWER_BIT: u32 = 11;
pub const KTRACE_GRP_OOM_BIT: u32 = 12;
// Keep last and updated as you add more bits.
pub const KTRACE_GRP_NEXT_UNUSED_BIT: u32 = 13;

pub const KTRACE_GRP_ALL: u32 = (1 << KTRACE_GRP_NEXT_UNUSED_BIT) - 1;
pub const KTRACE_GRP_META: u32 = 1 << KTRACE_GRP_META_BIT;
pub const KTRACE_GRP_MEMORY: u32 = 1 << KTRACE_GRP_MEMORY_BIT;
pub const KTRACE_GRP_SCHEDULER: u32 = 1 << KTRACE_GRP_SCHEDULER_BIT;
pub const KTRACE_GRP_CONTENTION: u32 = 1 << KTRACE_GRP_CONTENTION_BIT;
pub const KTRACE_GRP_IPC: u32 = 1 << KTRACE_GRP_IPC_BIT;
pub const KTRACE_GRP_IRQ: u32 = 1 << KTRACE_GRP_IRQ_BIT;
pub const KTRACE_GRP_PROBE: u32 = 1 << KTRACE_GRP_PROBE_BIT;
pub const KTRACE_GRP_ARCH: u32 = 1 << KTRACE_GRP_ARCH_BIT;
pub const KTRACE_GRP_SYSCALL: u32 = 1 << KTRACE_GRP_SYSCALL_BIT;
pub const KTRACE_GRP_VM: u32 = 1 << KTRACE_GRP_VM_BIT;
pub const KTRACE_GRP_RESTRICTED: u32 = 1 << KTRACE_GRP_RESTRICTED_BIT;
pub const KTRACE_GRP_POWER: u32 = 1 << KTRACE_GRP_POWER_BIT;
pub const KTRACE_GRP_OOM: u32 = 1 << KTRACE_GRP_OOM_BIT;

pub const KTRACE_ACTION_START: u32 = 1; // options = grpmask, 0 = all
pub const KTRACE_ACTION_STOP: u32 = 2; // options ignored
pub const KTRACE_ACTION_REWIND: u32 = 3; // options ignored
pub const KTRACE_ACTION_START_CIRCULAR: u32 = 5; // options = grpmask, 0 = all
