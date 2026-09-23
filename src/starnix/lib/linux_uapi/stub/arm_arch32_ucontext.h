// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_LINUX_UAPI_STUB_ARM_ARCH32_UCONTEXT_H_
#define SRC_STARNIX_LIB_LINUX_UAPI_STUB_ARM_ARCH32_UCONTEXT_H_

#include <asm/sigcontext.h>
#include <asm/signal.h>

typedef struct {
  unsigned long sig[_KERNEL_NSIG / (8 * sizeof(unsigned long))];
} sigset64_t;
typedef struct sigcontext mcontext_t;

typedef struct ucontext {
  unsigned long uc_flags;
  struct ucontext* uc_link;
  stack_t uc_stack;
  mcontext_t uc_mcontext;
  sigset64_t uc_sigmask64;
  uint8_t extended_pstate[4096];
} ucontext_t;

#if defined(sa_handler)
#undef sa_handler
#endif

typedef struct sigaction64 {
  __sighandler_t sa_handler;
  unsigned long sa_flags;
  void* sa_restorer;
  sigset64_t sa_mask;
} sigaction64_t;

#endif  // SRC_STARNIX_LIB_LINUX_UAPI_STUB_ARM_ARCH32_UCONTEXT_H_
