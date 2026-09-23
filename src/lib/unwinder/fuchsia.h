// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef __Fuchsia__
#error Fuchsia-only code
#endif

#ifndef SRC_LIB_UNWINDER_FUCHSIA_H_
#define SRC_LIB_UNWINDER_FUCHSIA_H_

#include <zircon/syscalls/debug.h>
#include <zircon/types.h>

#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

class FuchsiaMemory : public Memory {
 public:
  // The ownership of the process is not taken. The handle must outlast this object.
  explicit FuchsiaMemory(zx_handle_t process) : process_(process) {}

  // |Memory| implementation.
  Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override;

 private:
  zx_handle_t process_;
};

// Convert zx_thread_state_general_regs_t to Registers.
Registers FromFuchsiaRegisters(const zx_thread_state_general_regs_t& regs);

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_FUCHSIA_H_
