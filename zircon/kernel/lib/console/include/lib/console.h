// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2009 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_CONSOLE_INCLUDE_LIB_CONSOLE_H_
#define ZIRCON_KERNEL_LIB_CONSOLE_INCLUDE_LIB_CONSOLE_H_

#include <debug.h>
#include <lib/special-sections/special-sections.h>
#include <stdbool.h>
#include <stddef.h>
#include <sys/types.h>
#include <zircon/types.h>

#include <fbl/macros.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <kernel/timer.h>

struct cmd_args {
  const char* str;
  uint64_t u;
  void* p;
  int64_t i;
  bool b;
};

using console_cmd = int(int argc, const cmd_args* argv, uint32_t flags);

#define CMD_AVAIL_NORMAL (0x1 << 0)
#define CMD_AVAIL_PANIC (0x1 << 1)
#define CMD_AVAIL_ALWAYS (CMD_AVAIL_NORMAL | CMD_AVAIL_PANIC)

/* command is happening at crash time */
#define CMD_FLAG_PANIC (0x1 << 0)

/* a block of commands to register */
struct cmd {
  const char* cmd_str;
  const char* help_str;
  console_cmd* cmd_callback;
  uint8_t availability_mask;
};

/* register a static block of commands at init time */

#define CONSOLE_ENABLED (LK_DEBUGLEVEL > 0)

#if CONSOLE_ENABLED

#define STATIC_COMMAND_START \
  static const cmd _lk_cmd_list SPECIAL_SECTION(".data.rel.ro.commands", cmd)[] = {
#define STATIC_COMMAND_END(name) \
  }                              \
  ;

#define STATIC_COMMAND(command_str, help_str, func) {command_str, help_str, func, CMD_AVAIL_NORMAL},
#define STATIC_COMMAND_MASKED(command_str, help_str, func, availability_mask) \
  {command_str, help_str, func, availability_mask},

#else

#define STATIC_COMMAND_START [[maybe_unused]] static void _lk_cmd_list() {
#define STATIC_COMMAND_END(name) }
#define STATIC_COMMAND(command_str, help_str, func) (void)(func);
#define STATIC_COMMAND_MASKED(command_str, help_str, func, availability_mask) (void)(func);

#endif  // CONSOLE_ENABLED

#if CONSOLE_ENABLED

/* external api */
extern "C" int rust_console_run_script(const char* string);
extern "C" int rust_console_run_script_locked(
    const char* string);  // special case from inside a command
extern "C" void cpp_console_lock();
extern "C" void cpp_console_unlock();

/* panic shell api */
extern "C" void rust_panic_shell_start();

// Attempt to start the kernel shell.
// Will return if shell is not started or if shell exits.
extern "C" void rust_kernel_shell_init();

// FFIs
inline int console_run_script(const char* string) { return rust_console_run_script(string); }
inline int console_run_script_locked(const char* string) {
  return rust_console_run_script_locked(string);
}
inline void panic_shell_start() { rust_panic_shell_start(); }
inline void kernel_shell_init() { rust_kernel_shell_init(); }

#else

// Easier to link in stubbed definitions when the console is disabled.
inline int console_run_script(const char* string) { return 0; }
inline int console_run_script_locked(const char* string) { return 0; }
inline void panic_shell_start() {}
inline void kernel_shell_init() {}
#endif  // CONSOLE_ENABLED

#endif  // ZIRCON_KERNEL_LIB_CONSOLE_INCLUDE_LIB_CONSOLE_H_
