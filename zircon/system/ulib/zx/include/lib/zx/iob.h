// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZX_IOB_H_
#define LIB_ZX_IOB_H_

#include <lib/zx/handle.h>
#include <lib/zx/object.h>
#include <lib/zx/result.h>
#include <zircon/availability.h>
#include <zircon/syscalls/iob.h>

namespace zx {

class iob final : public object<iob> {
 public:
  static constexpr zx_obj_type_t TYPE = ZX_OBJ_TYPE_IOB;

  constexpr iob() = default;

  explicit iob(zx_handle_t value) : object(value) {}

  explicit iob(handle&& h) : object(h.release()) {}

  iob(iob&& other) : object(other.release()) {}

  iob& operator=(iob&& other) {
    reset(other.release());
    return *this;
  }

  static zx_status_t create(uint64_t options, zx_iob_region_t* regions, uint32_t region_count,
                            iob* endpoint0, iob* endpoint1) ZX_AVAILABLE_SINCE(14);

  // Perfms a mediated write to an IOBuffer region. See
  // https://fuchsia.dev/reference/syscalls/iob_writev.
  zx::result<> writev(zx_iob_write_options_t options, uint32_t region_index,
                      const zx_iovec_t* vector, size_t vector_count) const ZX_AVAILABLE_SINCE(28) {
    return zx::make_result(zx_iob_writev(get(), options, region_index, vector, vector_count));
  }
} ZX_AVAILABLE_SINCE(14);

typedef unowned<iob> unowned_iob ZX_AVAILABLE_SINCE(14);

}  // namespace zx

#endif  // LIB_ZX_IOB_H_
