// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/resource.h"

#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

extern "C" {
zx_status_t rust_resource_validate_ranged_resource(zx_handle_t handle, zx_rsrc_kind_t kind,
                                                   uint64_t base, size_t size, bool strict);
}

zx_status_t validate_ranged_resource(zx_handle_t handle, zx_rsrc_kind_t kind, uintptr_t base,
                                     size_t size, StrictMmioRangeValidation strict_validation) {
  return rust_resource_validate_ranged_resource(
      handle, kind, base, size, strict_validation == StrictMmioRangeValidation::Yes);
}
