// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSCALLS_RSEQ_H_
#define ZIRCON_SYSCALLS_RSEQ_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

// Defines the bounds of the restartable critical section.
typedef struct zx_rseq {
  // The CPU ID of the CPU on which the thread is currently executing.
  //
  // This field must never be written by userspace and must not be read on any
  // thread other than the thread that registered the restartable sequence.
  //
  // Userspace should initialize this field to ZX_INFO_INVALID_CPU. Zircon will
  // set this field to the current CPU ID when the restartable sequence is
  // registered.
  uint32_t cpu_id;

  // Reserved for future use. Userspace must initialize this field to zero.
  uint32_t reserved;

  // Either zero or the address of the first instruction in the
  // restartable sequence. This field must not be accessed on any thread other
  // than the thread that registered the restartable sequence.
  zx_vaddr_t start_ip;

  // Either zero or the offset from the `start_ip` to the address after the
  // last instruction in the restartable sequence. This field must not be
  // accessed on any thread other than the thread that registered the
  // restartable sequence.
  size_t post_commit_offset;

  // Either zero or the instruction pointer at which the kernel should resume
  // the thread if it preempts the thread while the thread is executing the
  // restartable sequence. This field must not be accessed on any thread other
  // than the thread that registered the restartable sequence.
  zx_vaddr_t abort_ip;
} __ALIGNED(32) zx_rseq_t;

#endif  // ZIRCON_SYSCALLS_RSEQ_H_
