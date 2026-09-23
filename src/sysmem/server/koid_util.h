// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_KOID_UTIL_H_
#define SRC_SYSMEM_SERVER_KOID_UTIL_H_

#include <lib/fpromise/result.h>
#include <lib/zx/channel.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <vector>

#include "src/sysmem/server/macros.h"

namespace sysmem_service {

zx_status_t get_handle_koids(const zx::object_base& this_end, zx_koid_t* this_end_koid,
                             zx_koid_t* that_end_koid, zx_obj_type_t type);

template <class T>
zx_koid_t get_koid(const zx::object<T>& object) {
  zx_koid_t this_end_koid;
  zx_koid_t that_end_koid;
  zx_status_t result = get_handle_koids(object, &this_end_koid, &that_end_koid, T::TYPE);
  if (result != ZX_OK) {
    return ZX_KOID_INVALID;
  }
  return this_end_koid;
}

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_KOID_UTIL_H_
