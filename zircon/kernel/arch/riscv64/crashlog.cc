// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <stdio.h>

#include <arch/crashlog.h>

extern "C" void rust_arch_render_crashlog_registers(void (*write_cb)(void*, const char*, size_t),
                                                    void* ctx, const crashlog_regs_t* regs);

void arch_render_crashlog_registers(FILE& target, const crashlog_regs_t& regs) {
  auto write_fn = [](void* ctx, const char* str, size_t len) {
    auto* f = static_cast<FILE*>(ctx);
    fwrite(str, 1, len, f);
  };
  rust_arch_render_crashlog_registers(write_fn, &target, &regs);
}
