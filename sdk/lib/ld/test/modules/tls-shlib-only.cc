// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/machine.h>
#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>

#include "test-start.h"
#include "tls-dep.h"

using Traits = elfldltl::TlsTraits<>;

constexpr size_t kExpectedOffset =
    Traits::kTlsNegative ? -kTlsDepAlignedTotalSize
                         : ((Traits::kTlsLocalExecOffset + kTlsDepAlign - 1) & -kTlsDepAlign);

constexpr size_t kExpectedSize =
    Traits::kTlsNegative ? -kExpectedOffset : kExpectedOffset + kTlsDepTotalSize;

extern "C" int64_t TestStart() {
  const auto modules = ld::AbiLoadedModules(ld::abi::_ld_abi);
  const auto& exec_module = *modules.begin();
  const auto& shlib_module = FindTlsDep(modules);

  if (exec_module.tls_modid != 0) {
    return 1;
  }

  if (shlib_module.tls_modid != 1) {
    return 2;
  }

  if (ld::abi::_ld_abi.static_tls_modules.size() != 1) {
    return 3;
  }

  const auto& shlib_tls = ld::abi::_ld_abi.static_tls_modules.front();

  if (shlib_tls.tls_initial_data.size_bytes() != sizeof(tls_dep_data)) {
    return 3;
  }

  if (*reinterpret_cast<const int*>(shlib_tls.tls_initial_data.data()) != kTlsDepDataValue) {
    return 4;
  }

  if (shlib_tls.tls_bss_size != kTlsDepBssSize) {
    return 5;
  }

  if (shlib_tls.tls_alignment != kTlsDepAlign) {
    return 6;
  }

  if (ld::abi::_ld_abi.static_tls_offsets.size() != 1) {
    return 7;
  }

  if (ld::abi::_ld_abi.static_tls_offsets.front() != kExpectedOffset) {
    return 8;
  }

  if (ld::abi::_ld_abi.static_tls_layout.alignment() != kTlsDepAlign) {
    return 9;
  }

  if (ld::abi::_ld_abi.static_tls_layout.size_bytes() != kExpectedSize) {
    return 10;
  }

  return 17;
}
