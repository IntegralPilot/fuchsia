// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_STARTUP_STARTUP_RELOCATE_H_
#define LIB_C_STARTUP_STARTUP_RELOCATE_H_

#include <lib/zx/vmar.h>

#include <cstddef>
#include <cstdint>

#include "../asm-linkage.h"
#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// Just constructing this object performs self-relocation for a static PIE.
// Once it's constructed, system calls can be used.  As soon as the VMAR handle
// covering the executable's image is available, its method should be called to
// apply RELRO protections.  In the shared C library, the both are no-ops.
class [[nodiscard]] StartupRelocate {
 public:
  StartupRelocate(const StartupRelocate&) = delete;
  StartupRelocate(StartupRelocate&&) = default;

  static StartupRelocate Create(const void* vdso_base)
      LIBC_ASM_LINKAGE_DECLARE(StartupRelocateCreate);

  void ProtectRelro(zx::vmar loaded_vmar) const&& LIBC_ASM_LINKAGE_DECLARE(
      StartupRelocateProtectRelro);

 private:
  StartupRelocate() = default;

  uintptr_t start_ = 0;
  size_t size_ = 0;
};

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_STARTUP_STARTUP_RELOCATE_H_
