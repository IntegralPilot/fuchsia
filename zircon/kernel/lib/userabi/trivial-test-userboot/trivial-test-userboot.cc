// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/syscalls.h>

#include <__verbose_abort>
#include <string_view>

#include "fuchsia-static-pie.h"

using namespace std::literals;

namespace {

char big_bss[128 << 10];

void DebugWrite(std::string_view s) {
  zx_status_t status = zx_debug_write(s.data(), s.size());
  if (status != ZX_OK) {
    __builtin_trap();
  }
}

[[noreturn]] void Panic(std::string_view str) {
  DebugWrite(str);
  __builtin_trap();
}

}  // namespace

extern "C" [[noreturn]] void _start(zx_handle_t bootstrap, const void* vdso) {
  StaticPieSetup(vdso);

  // Give the kernel time to drain the debuglog before we produce any output.
  zx_nanosleep(zx_deadline_after(ZX_SEC(3)));

  static int data_location;
  static int* volatile data_address = &data_location;
  static int* const volatile relro_address = &data_location;

  // This makes absolutely sure the compiler doesn't think it knows how to
  // optimize away the fetches and tests.
  int* from_data;
  __asm__("" : "=g"(from_data) : "0"(data_address));

  // Since data_location has internal linkage, the references here will use
  // pure PC-relative address materialization.

  if (from_data != &data_location) {
    Panic("address in data not relocated properly"sv);
  }

  int* from_relro;
  __asm__("" : "=g"(from_relro) : "0"(relro_address));
  if (from_relro != &data_location) {
    Panic("address in RELRO not relocated properly"sv);
  }

  DebugWrite("Hello, world!\n"sv);

  DebugWrite("Let's fill some .bss!\n"sv);
  char* in_bss;
  __asm__("" : "=g"(in_bss) : "0"(big_bss));
  memset(in_bss, 0xaa, sizeof(big_bss));

  DebugWrite("\n+++ TEST PASSED! +++\n\n"sv);
  DebugWrite(BOOT_TEST_SUCCESS_STRING "\n");
  zx_process_exit(0);
}

// Inline functions in libc++ headers call this.
[[noreturn]] void std::__libcpp_verbose_abort(const char* format, ...) noexcept {
  Panic("libc++ abort, message lost"sv);
}
