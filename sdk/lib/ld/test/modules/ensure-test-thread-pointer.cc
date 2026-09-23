// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ensure-test-thread-pointer.h"

#include <stdint.h>
#include <zircon/compiler.h>

#if defined(__x86_64__) && defined(__linux__)
#include <asm/prctl.h>
#include <asm/unistd.h>
#endif

// Make sure that accessing the thread pointer in the normal way is valid, such
// that forming the address of a `thread_local` variable will work.  Note this
// does not mean that the address so formed will be valid for memory access.
// It means only that it will be valid to take the address of a variable via
// the compiler and it will be valid to use ld::TpRelative() so the offset
// between the two can be calculated.
bool EnsureTestThreadPointer() {
  // On most machines it doesn't matter if the thread pointer has been "set up"
  // because it can always be fetched, even if its value is just zero.  For
  // comparing offsets a value of zero is no worse than any other.  When doing
  // in-process tests, the thread pointer was already set up by the normal
  // system threading support in the gtest program so there is nothing to do.
#if !defined(__x86_64__) || defined(IN_PROCESS_TEST)
  return true;
#elif defined(__linux__)
  // The arch_prctl multiplexor system call has ARCH_SET_FS to set %fs.base.
  // It's simpler to wire up the syscall by hand here than it is to use
  // linux_syscall_support.h as linux-syscalls.cc does.
  struct FsAbi {
    FsAbi* self = this;
  };
  __CONSTINIT static FsAbi fs_points_to;
  int64_t result;
  __asm__ volatile("syscall"
                   : "=a"(result)
                   : "0"(__NR_arch_prctl), "D"(ARCH_SET_FS), "S"(&fs_points_to)
                   // The kernel doesn't touch any registers except the result,
                   // but the SYSCALL instruction in the CPU itself always
                   // clobbers %rcx and %r11.
                   : "rcx", "r11");
  if (result == 0) {
    return true;
  }
  __builtin_trap();
#else
  // On Fuchsia/x86-64, setting %fs.base requires the thread handle, which is
  // not easily accessible to the standalone test code.
  return false;
#endif
}
