// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_ZIRCON_ZX_NAME_H_
#define LIB_C_ZIRCON_ZX_NAME_H_

#include <lib/zx/object.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <array>
#include <cassert>
#include <concepts>
#include <string_view>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

class ZxName {
 public:
  constexpr ZxName() = default;
  constexpr ZxName(const ZxName&) = default;

  [[gnu::format(printf, 1, 2)]] static ZxName Printf(const char* fmt, ...);

  constexpr explicit ZxName(std::string_view name) {
    // name_.back() and any earlier unused chars are already '\0'.
    name.copy(name_.data(), name_.size() - 1);
  }

  constexpr ZxName& operator=(const ZxName&) = default;

  constexpr bool empty() const { return name_.front() == '\0'; }

  constexpr std::string_view str() const {
    std::string_view name{name_.data(), name_.size() - 1};
    return name.substr(0, name.find_first_of('\0'));
  }

  constexpr const char* c_str() const {
    assert(name_.back() == '\0');
    return name_.data();
  }

  static zx::result<ZxName> Get(const std::derived_from<zx::object_base> auto& handle) {
    ZxName name;
    zx_status_t status = handle.get_property(ZX_PROP_NAME, name.name_.data(), name.name_.size());
    return zx::make_result(status, name);
  }

  zx::result<> Set(const std::derived_from<zx::object_base> auto& handle) const {
    return zx::make_result(handle.set_property(ZX_PROP_NAME, name_.data(), name_.size()));
  }

 private:
  std::array<char, ZX_MAX_NAME_LEN> name_{};
};

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_ZIRCON_ZX_NAME_H_
