// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>

#include "ensure-test-thread-pointer.h"
#include "test-start.h"
#include "tls-dep.h"

[[gnu::used, gnu::retain]] alignas(64) constinit thread_local int tls_data = 23;
[[gnu::used, gnu::retain]] constinit thread_local char tls_bss[2];

using Traits = elfldltl::TlsTraits<>;

constexpr size_t kExpectedAlign = 64;
static_assert(kExpectedAlign >= kTlsDepAlign);

constexpr size_t kAlignedExecOffset =
    Traits::kTlsLocalExecOffset == 0
        ? 0
        : (Traits::kTlsLocalExecOffset + kExpectedAlign - 1) & -kExpectedAlign;

constexpr size_t kExecOffset = Traits::kTlsNegative ? -kExpectedAlign : kAlignedExecOffset;

constexpr size_t kShlibOffset =
    Traits::kTlsNegative ? -(kExpectedAlign + kTlsDepAlignedTotalSize)
                         : ((kExecOffset + sizeof(int) * 2 + kTlsDepAlign - 1) & -kTlsDepAlign);

constexpr size_t kExpectedSize = Traits::kTlsNegative ? kExpectedAlign + kTlsDepAlignedTotalSize
                                                      : kShlibOffset + kTlsDepTotalSize;

constexpr ptrdiff_t kTpOffsetForDepData = std::bit_cast<ptrdiff_t>(kShlibOffset);
constexpr ptrdiff_t kTpOffsetForDepBss = std::bit_cast<ptrdiff_t>(kShlibOffset + kTlsDepAlign);

extern "C" int64_t TestStart() {
  const auto modules = ld::AbiLoadedModules(ld::abi::_ld_abi);

  const auto& exec_module = *modules.begin();
  const auto& shlib_module = FindTlsDep(modules);

  if (exec_module.tls_modid != 1) {
    return 1;
  }

  if (shlib_module.tls_modid != 2) {
    return 2;
  }

  if (ld::abi::_ld_abi.static_tls_modules.size() != 2) {
    return 3;
  }

  const auto& exec_tls = ld::abi::_ld_abi.static_tls_modules.front();
  const auto& shlib_tls = ld::abi::_ld_abi.static_tls_modules.back();

  if (exec_tls.tls_initial_data.size_bytes() != sizeof(tls_data)) {
    return 4;
  }

  if (shlib_tls.tls_initial_data.size_bytes() != sizeof(tls_dep_data)) {
    return 5;
  }

  if (*reinterpret_cast<const int*>(exec_tls.tls_initial_data.data()) != 23) {
    return 6;
  }

  if (*reinterpret_cast<const int*>(shlib_tls.tls_initial_data.data()) != kTlsDepDataValue) {
    return 7;
  }

  if (exec_tls.tls_bss_size != sizeof(tls_bss)) {
    return 8;
  }

  if (shlib_tls.tls_bss_size != kTlsDepBssSize) {
    return 9;
  }

  if (exec_tls.tls_alignment != 64) {
    return 10;
  }

  if (shlib_tls.tls_alignment != kTlsDepAlign) {
    return 11;
  }

  if (ld::abi::_ld_abi.static_tls_offsets.size() != 2) {
    return 12;
  }

  if (ld::abi::_ld_abi.static_tls_offsets.front() != kExecOffset) {
    return 13;
  }

  if (ld::abi::_ld_abi.static_tls_offsets.back() != kShlibOffset) {
    return 14;
  }

  if (ld::abi::_ld_abi.static_tls_layout.alignment() != kExpectedAlign) {
    return 15;
  }

  if (ld::abi::_ld_abi.static_tls_layout.size_bytes() != kExpectedSize) {
    return 16;
  }

  if (ld::TlsInitialExecOffset(ld::abi::_ld_abi, 2) != kTpOffsetForDepData) {
    return 18;
  }

  int tls_block[2] = {123, 456};
  ld::TlsModuleInit(exec_tls, std::as_writable_bytes(std::span(tls_block)));
  if (tls_block[0] != 23) {
    return 19;
  }
  if (tls_block[1] != 0) {
    return 20;
  }

  if (EnsureTestThreadPointer()) {
    // The compiler will emit IE accesses here and the dynamic linker will fill
    // in the GOT entries with offsets at startup.  Verify that the offsets
    // match what we expect.

    if (ld::TpRelativeToOffset(&tls_dep_data) != kTpOffsetForDepData) {
      return 21;
    }

    // Ideally this would generate an IE reloc with an addend, but in fact both
    // compilers seem never to do that and always just generate a reloc to the
    // symbol and then add 1 to the materialized pointer.
    if (ld::TpRelativeToOffset(&tls_dep_bss[1]) != kTpOffsetForDepBss + 1) {
      return 22;
    }
  }

  return 17;
}
