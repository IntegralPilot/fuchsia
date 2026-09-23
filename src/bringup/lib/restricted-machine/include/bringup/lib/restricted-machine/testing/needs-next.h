// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file declares the system calls needed for restricted_machine:: use from
// the @next system calls.  This should only be needed for
// //zircon/system/utest/core testing, but we have to proactively mark the
// syscalls weak for inclusion into a stable-vdso binary.
#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_NEEDS_NEXT_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_NEEDS_NEXT_H_

#include <zircon/syscalls-next.h>
// See //zircon/system/utest/core/needs-next.h for details.
#ifndef NEEDS_NEXT_SYSCALL
#define NEEDS_NEXT_SYSCALL(name) [[gnu::weak]] decltype(name) name
#endif

// Declare the system calls needed.
NEEDS_NEXT_SYSCALL(zx_restricted_bind_state);
NEEDS_NEXT_SYSCALL(zx_restricted_enter);
NEEDS_NEXT_SYSCALL(zx_restricted_kick);
NEEDS_NEXT_SYSCALL(zx_restricted_unbind_state);

// Provide a single call to skip a test for all of the above.
#define RM_NEEDS_NEXT_SKIP                   \
  NEEDS_NEXT_SKIP(zx_restricted_bind_state); \
  NEEDS_NEXT_SKIP(zx_restricted_enter);      \
  NEEDS_NEXT_SKIP(zx_restricted_kick);       \
  NEEDS_NEXT_SKIP(zx_restricted_unbind_state);

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_NEEDS_NEXT_H_
