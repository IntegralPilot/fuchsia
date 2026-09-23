// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <tuple>

#include "startup-relocate.h"

// These are the do-nothing stubs for the shared-library version of
// StartupRelocate.  There is nothing to do because the startup dynamic linker
// already did it all.

namespace LIBC_NAMESPACE_DECL {

// The members have initializers in the declarations just for cleanliness.
// With LTO, setting them here can be optimized away because they're unused.
StartupRelocate StartupRelocate::Create(const void* vdso_base) { return {}; }

void StartupRelocate::ProtectRelro(zx::vmar loaded_vmar) const&& {
  // Consuming them here silences -Wunused-private-field and should still let
  // them be optimized away entirely with LTO.
  std::ignore = start_;
  std::ignore = size_;
}

}  // namespace LIBC_NAMESPACE_DECL
