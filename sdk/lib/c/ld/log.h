// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_LD_LOG_H_
#define LIB_C_LD_LOG_H_

#include <lib/ld/log-zircon.h>

#include <atomic>

#include "../asm-linkage.h"
#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// These methods are only visible at link time inside the hermetic_source_set()
// for __sanitizer_log_write.  Calls outside that can use the ld::Log methods,
// which will be separately compiled inside and outside that hermetic link.
class Log : public ld::Log {
 public:
  // Print all the symbolzer context not already printed.
  void SymbolizerContext();

  // This indicates the symbolizer context was already printed by the startup
  // dynamic linker, so startup modules don't need to be logged again.
  void StartupSymbolizerContext();

 private:
  std::atomic_flag context_logged_;
};

// The gLog instance is defined outside the hermetic_source_set(), so it needs
// custom name mangling to allow the reference across the hermetic boundary.
// The instance's ld::Log methods can be used freely in the rest of libc, and
// they must be used there to set its handles at startup.
[[gnu::visibility("hidden")]] extern Log gLog LIBC_ASM_LINKAGE_DECLARE(gLog);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_LD_LOG_H_
