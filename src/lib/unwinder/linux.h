// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef __linux__
#error Linux-only code
#endif

#ifndef SRC_LIB_UNWINDER_LINUX_H_
#define SRC_LIB_UNWINDER_LINUX_H_

#include <sys/types.h>
#include <sys/user.h>

#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

class LinuxMemory : public Memory {
 public:
  // The ownership of the process is not taken. The handle must outlast this object.
  explicit LinuxMemory(pid_t process) : pid_(process) {}

  // |Memory| implementation.
  Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override;

 private:
  pid_t pid_;
};

// Convert user_regs_struct to Registers.
Registers FromLinuxRegisters(const struct user_regs_struct& regs);

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_LINUX_H_
