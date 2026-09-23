// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/console.h>
#include <zircon/compiler.h>

#include <kernel/mutex.h>
#include <lk/init.h>

#if CONSOLE_ENABLED

namespace {
DECLARE_SINGLETON_MUTEX(CommandLock);
}  // namespace

extern "C" {

void cpp_console_lock() TA_NO_THREAD_SAFETY_ANALYSIS { CommandLock::Get()->lock().Acquire(); }

void cpp_console_unlock() TA_NO_THREAD_SAFETY_ANALYSIS { CommandLock::Get()->lock().Release(); }

void rust_console_init_history(void);

}  // extern "C"

static void console_init(uint level) { rust_console_init_history(); }

LK_INIT_HOOK(console, console_init, LK_INIT_LEVEL_HEAP)

#endif  // CONSOLE_ENABLED
