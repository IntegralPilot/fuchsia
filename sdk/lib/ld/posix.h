// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_POSIX_H_
#define LIB_LD_POSIX_H_

#include <lib/elfldltl/mmap-loader.h>
#include <lib/ld/log-posix.h>

#include <array>
#include <cstddef>
#include <cstdint>

#include "startup-load.h"

namespace ld {

using StartupModule = StartupLoadModule<elfldltl::MmapLoader<>>;

// The auxiliary vector on the stack is a sequence of tag, value pairs.
using Auxv = std::array<uintptr_t, 2>;

// On entry, the SP points to this.
struct StartupStack {
  uintptr_t argc;
  char* argv[];
  // After argv[argc] is nullptr, then envp[...] terminated by nullptr.
  // After that is the auxv, which is pairs of tag, value (uintptr_t)
  // words, terminated by the AT_NULL (zero) tag.

  constexpr char** envp() { return &argv[argc] + 1; }

  const Auxv* GetAuxv() {
    char** ep = envp();
    while (*ep != nullptr) {
      ++ep;
    }
    return reinterpret_cast<const Auxv*>(ep + 1);
  }
};

// These are the AT_* tag values of potential interest here.
enum class AuxvTag : uintptr_t {
  kNull = 0,
  kExecFd = 2,
  kPhdr = 3,
  kPhent = 4,
  kPhnum = 5,
  kPagesz = 6,
  kEntry = 9,
  kSysinfoEhdr = 33,
};

// This collects the data gleaned from the initial stack and the auxv on it.
struct StartupData {
  uintptr_t argc = 0;
  char** argv = nullptr;
  char** envp = nullptr;

  size_t page_size = 0;

  bool ld_debug = false;

  [[no_unique_address]] Log log;
};

}  // namespace ld

#endif  // LIB_LD_POSIX_H_
