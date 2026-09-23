// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_STARTUP_STARTUP_TRAMPOLINE_H_
#define LIB_C_STARTUP_STARTUP_TRAMPOLINE_H_

#include <zircon/types.h>

#include <cstdint>

#include "../asm-linkage.h"
#include "src/__support/macros/config.h"

// This is [phase one](README.md#executable-entry) of the libc _per se_
// (i.e. libc.a or libc.so) part in program startup: immediately after the
// [_start](crt1.S) code, possibly after a startup or remote dynamic linker.
// It's called the startup trampoline because it "bounces" from phase one
// (basic machine ABI) to phase two (full Fuchsia Compiler ABI on new stacks):
// from the first (assembly) [`__libc_start_main`](startup-trampoline.S) to the
// second (C++) [`__libc_start_main`](start-main.cc).

namespace LIBC_NAMESPACE_DECL {

// This is the full signature of the `main` function as it's actually called by
// Unix tradition.  The C standard actually allows either of two signatures for
// the application's main function, and neither of them is this one.  It allows
// zero arguments or two, but passing a third argument is always harmless in
// all the calling conventions actually in use (just like passing the first two
// the standard specifies is harmless to the allowed zero-argument definition).
//
// It can't be checked by CFI.  Even if it were assumed in the given libc build
// that every user's main function will necessarily have been compiled with CFI
// instrumentation, there are three different valid signatures the user might
// have used; but CFI would require that it match the single one declared here.
using MainFunction =  //
    int(int argc, char** argv, char** envp) [[clang::cfi_unchecked_callee]];

// This is the ABI between the _start (Scrt1.o) code linked into each
// executable and libc, whether libc is linked in statically or dynamically.
// This is implemented in assembly (startup-trampoline.S) and is wholly
// independent of the bootstrap protocol details.  It is entered using the
// basic machine ABI and calls StartCompilerAbi (below), but then acts as a
// tail call to the namespaced __libc_start_main (declared in start-main.h).
//
// Both functions have the same name in the source and thus as rendered in
// backtraces with debugging symbols so that, at least by plain function name,
// every point in startup should show _start -> __libc_start_main -> ... in a
// backtrace whether in the initial phase or when -> ... is -> main -> ...
//
// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern "C" [[noreturn]] void __libc_start_main(
    // The first two arguments are passed by zx_process_start.
    zx_handle_t bootstrap_client_end, const void* vdso,
    // The third is only passed (nonzero) by the dynamic linker.
    zx_handle_t svc_server_end,
    // The fourth is always set in the _start code's call directly.
    MainFunction*);

// StartCompilerAbi returns this.  The trampoline assembly code expects it as
// packed into the two return value registers.
struct StartupTrampoline {
  void* hook;    // zx_startup_handles_t::hook value.
  uint64_t* sp;  // Initial SP: off the end of the stack, always aligned to 16.
};
// This must be exactly two words to be returned in two registers.
static_assert(sizeof(StartupTrampoline) == 2 * sizeof(void*));

// This is called by startup-trampoline.S using the basic machine ABI.  It
// takes exactly the arguments of the actual `zx_process_start` entry point
// (the first two that `_start` and then `__libc_start_main` saw and then
// propagated), but returns a value (in two registers).
//
// It's responsible for allocating the stacks and initializing the thread
// pointer.  It uses [`_zx_startup_get_handles`](../include/zircon/startup.h).
// That must decode the bootstrap protocol at least enough to acquire essential
// handles like the VMAR to use for allocation.  This does all the allocation
// and initializes the thread pointer and shadow call stack register as fully
// as required by the Fuchsia Compiler ABI.  When it returns to the assembly
// code, [phase two](start-main.h) is entered.
StartupTrampoline StartCompilerAbi(zx_handle_t bootstrap_client_end, const void* vdso)
    LIBC_ASM_LINKAGE_DECLARE(StartCompilerAbi);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_STARTUP_STARTUP_TRAMPOLINE_H_
