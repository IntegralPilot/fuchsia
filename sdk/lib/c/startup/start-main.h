// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_STARTUP_START_MAIN_H_
#define LIB_C_STARTUP_START_MAIN_H_

#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>

#include "../asm-linkage.h"
#include "../zircon/vmar.h"
#include "src/__support/macros/config.h"
#include "startup-trampoline.h"

namespace LIBC_NAMESPACE_DECL {

// Get the stack size indicated by the executable's PT_GNU_STACK, or default.
// This must use only the basic machine ABI because it's called in phase one
// from StartCompilerAbi (startup-trampoline.h).
PageRoundedSize InitialStackSize()
    // TODO(https://fxbug.dev/342469121): The asm linkage is only needed while
    // the legacy and new implementations coexist in the build.  This function
    // will be directly included into the hermetic_source_set() when there
    // aren't two different versions of it.
    LIBC_ASM_LINKAGE_DECLARE(InitialStackSize);

// This is called by StartCompilerAbi with the full Fuchsia Compiler ABI but
// still on the original stack: so technically still phase one, but closer to
// phase two and outside phase one's basic-ABI hermetic partial links so it
// must use asm linkage to be called across that hermetic boundary.  It takes
// ownership of these handles by storing them in libc global state that can't
// be accessed directly from hermetic (phase one proper) code.
void SetStartHandles(zx::process process_self, zx::vmar allocation_vmar, zx::thread thread_self)
    LIBC_ASM_LINKAGE_DECLARE(SetStartHandles);

// This is called by startup-trampoline.S (the public __libc_start_main) after
// switching to the stack returned by StartCompilerAbi.  It constitutes phase
// two: it already has the full Fuchsia Compiler ABI in place.  It calls the
// second two <zircon/startup.h> API functions that complete the work done in
// phase one to start handling the process bootstrap protocol and yield `hook`,
// which is propagated to the phase-two <zircon/startup.h> functions.
[[noreturn]] void __libc_start_main(  // NOLINT(bugprone-reserved-identifier)
    void* hook, zx_handle_t svc_server_end, MainFunction* main)
    LIBC_ASM_LINKAGE_DECLARE(start_main);

// This calls the <zircon/sanitizer.h> __sanitizer_module_loaded hook for every
// module dl_iterate_phdr would report.  This is called in phase two before
// calling the __sanitizer_startup_hook.
void StartupSanitizerModuleLoaded();

// This calls all the static constructors and such encoded via ELF.  This is
// the end of phase two, the point at which the full normal standard C runtime
// environment is entirely place: it comes after all sanitizer hooks; the
// libc-internal (and fdio) setup and preinit hooks; and very shortly before
// calling the user's `main` (final argument to __libc_start_main)
void StartupCtors();

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_STARTUP_START_MAIN_H_
