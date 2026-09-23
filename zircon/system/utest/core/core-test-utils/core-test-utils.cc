// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/core-test-utils.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/vmo.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <bit>
#include <optional>
#include <string>
#include <string_view>

#include <zxtest/zxtest.h>

// TODO(https://fxbug.dev/363254896): The Cavium hardware is believed to be
// buggy in occasional cross-socket multi-core races such that tests dependent
// on hardware clock reads can fail.

namespace core_test_utils {
namespace {

#ifdef __aarch64__
constexpr std::string_view kCaviumMidr = "Cavium CN99XX";
#else
constexpr std::string_view kCaviumMidr{};
#endif

// Returns std::nullopt if no midr.txt was present (as distinguished from the
// empty string, which indicates that it was present and empty).
std::optional<std::string> ReadMidrTxt() {
  std::string midr;
  zx::unowned_vmo midr_vmo = standalone::GetVmo("midr.txt");
  if (!*midr_vmo) {
    return std::nullopt;
  }

  uint64_t size = 0;
  EXPECT_OK(midr_vmo->get_prop_content_size(&size));
  midr.resize(size);
  EXPECT_OK(midr_vmo->read(midr.data(), 0, midr.size()));
  EXPECT_STRNE(midr, "");
  printf("INFO: Contents of midr.txt: \"%s\"\n", midr.c_str());
  return midr;
}

// Returns std::nullopt if we were unable to determine for certain whether we
// are on a Cavium (i.e., if no midr.txt was present on arm64).
[[maybe_unused]] std::optional<bool> CheckIsCavium() {
  if (kCaviumMidr.empty()) {
    return false;
  }

  // No midr.txt, so cannot determine Cavium-ness at this stage.
  std::optional<std::string> midr = ReadMidrTxt();
  if (!midr) {
    return std::nullopt;
  }
  return midr->starts_with(kCaviumMidr);
}

}  // namespace

// Returns std::nullopt if skipping should not occur, or else will return the
// message that should be passed to ZXTEST_SKIP().
std::optional<std::string_view> SkipBug363254896() {
#ifdef __aarch64__
  static std::optional<bool> is_cavium = CheckIsCavium();

  if (!is_cavium.has_value()) {
    return "WARNING: No midr.txt was present, so assuming the worst (i.e., that "
           "we're on a Cavium) and skipping per https://fxbug.dev/363254896\n";
  }
  if (*is_cavium) {
    return "We are on a Cavium, so skipping per https://fxbug.dev/363254896\n";
  }
#endif
  return std::nullopt;
}

#ifdef __x86_64__
void X86LowestNonCanonicalAddr(uintptr_t& out) {
  zx_info_vmar_t vmar_info;
  ASSERT_OK(zx_object_get_info(zx_vmar_root_self(), ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info),
                               nullptr, nullptr));
  // The end of the user's would-be address space may have been reserved (e.g.,
  // for "sysret" protection schemes), which means that the end of the VMAR may
  // be strictly smaller than the conventional hardware limit. We can recover
  // the latter however by taking the minimal power of 2 address greater or
  // equal to the computed end.
  out = std::bit_ceil(vmar_info.base + vmar_info.len);
}
#endif

}  // namespace core_test_utils
