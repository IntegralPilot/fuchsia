// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>

#include <cstdint>
#include <iterator>
#include <string_view>

#include "test-start.h"

extern "C" int64_t TestStart() {
  constexpr auto within_module = [](const auto& module, auto* ptr) {
    const uintptr_t addr = reinterpret_cast<uintptr_t>(ptr);
    return addr >= module.vaddr_start && addr < module.vaddr_end;
  };

  const auto modules = ld::AbiLoadedModules(ld::abi::_ld_abi);
  if (modules.empty()) {
    return 0;
  }

  const auto& first = *modules.begin();
  if (!std::string_view(first.link_map.name.get()).empty()) {
    return 1;
  }
  if (!within_module(first, &TestStart)) {
    return 2;
  }

  // The next module should be ld.so itself.
  auto it = std::next(modules.begin());
  if (it == modules.end()) {
    return 3;
  }

  const auto& second = *it++;
  if (second.soname != ld::abi::Abi<>::kSoname) {
    return 4;
  }
  if (!within_module(second, &ld::abi::_ld_abi)) {
    return 5;
  }

  size_t additional_modules = std::distance(it, modules.end());
  // There should be at most only 1 additional module. The vDSO may or may not
  // be present, but no additional modules should be found.
  if (additional_modules > 1) {
    return 6;
  }

  if (!ld::abi::_ld_abi.static_tls_modules.empty()) {
    return 7;
  }

  if (ld::abi::_ld_abi.loaded_modules_count != 2 + additional_modules) {
    return 8;
  }

  return 17;
}
