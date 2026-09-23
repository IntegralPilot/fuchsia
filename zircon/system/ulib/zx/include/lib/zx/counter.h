// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZX_COUNTER_H_
#define LIB_ZX_COUNTER_H_

#include <lib/zx/handle.h>
#include <lib/zx/object.h>

namespace zx {

class counter final : public object<counter> {
 public:
  static constexpr zx_obj_type_t TYPE = ZX_OBJ_TYPE_COUNTER;

  constexpr counter() = default;

  explicit counter(zx_handle_t value) : object(value) {}

  explicit counter(handle&& h) : object(h.release()) {}

  counter(counter&& other) : object(other.release()) {}

  counter& operator=(counter&& other) {
    reset(other.release());
    return *this;
  }

  static zx_status_t create(uint32_t options, counter* result);

  zx_status_t add(int64_t value) const { return zx_counter_add(get(), value); }
  zx_status_t read(int64_t* value) const { return zx_counter_read(get(), value); }
  zx_status_t write(int64_t value) const { return zx_counter_write(get(), value); }

} ZX_AVAILABLE_SINCE(HEAD);

typedef unowned<counter> unowned_counter ZX_AVAILABLE_SINCE(HEAD);

}  // namespace zx

#endif  // LIB_ZX_COUNTER_H_
