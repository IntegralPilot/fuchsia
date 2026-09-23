// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SYSCALLS_INCLUDE_LIB_SYSCALLS_FORWARD_H_
#define ZIRCON_KERNEL_LIB_SYSCALLS_INCLUDE_LIB_SYSCALLS_FORWARD_H_

#include <lib/user_copy/user_ptr.h>
#include <zircon/syscalls/types.h>
#include <zircon/types.h>

#include <object/handle.h>
#include <object/process_dispatcher.h>

// One of these macros is invoked by kernel.inc for each syscall.

// These don't have kernel entry points.
#define VDSO_SYSCALL(...)

// These are the direct kernel entry points.
#define KERNEL_SYSCALL(name, type, attrs, nargs, arglist, prototype) \
  attrs type sys_##name prototype;
#define INTERNAL_SYSCALL(...) KERNEL_SYSCALL(__VA_ARGS__)
#define BLOCKING_SYSCALL(...) KERNEL_SYSCALL(__VA_ARGS__)

#ifdef __clang__
#define _ZX_SYSCALL_ANNO(anno) __attribute__((anno))
#else
#define _ZX_SYSCALL_ANNO(anno)
#endif

#ifdef __cplusplus
#define _ZX_SYSCALL_EXTERN_C extern "C"
#else
#define _ZX_SYSCALL_EXTERN_C
#endif

#include <lib/syscalls/kernel.inc>

#undef VDSO_SYSCALL
#undef KERNEL_SYSCALL
#undef INTERNAL_SYSCALL
#undef BLOCKING_SYSCALL
#undef _ZX_SYSCALL_EXTERN_C
#undef _ZX_SYSCALL_ANNO

#endif  // ZIRCON_KERNEL_LIB_SYSCALLS_INCLUDE_LIB_SYSCALLS_FORWARD_H_
