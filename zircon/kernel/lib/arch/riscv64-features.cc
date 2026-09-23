// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/riscv64/feature.h>
#include <zircon/assert.h>

#include <string_view>

namespace arch {

RiscvFeatures& RiscvFeatures::SetMany(std::string_view isa_string) {
  using namespace std::string_view_literals;

  auto next_token = [&isa_string]() -> std::string_view {
    size_t pos = isa_string.find('_');
    if (pos == std::string_view::npos) {  // Last token.
      return std::exchange(isa_string, {});
    }
    std::string_view token = isa_string.substr(0, pos);
    isa_string.remove_prefix(pos + 1);
    return token;
  };

  // Parses a decimal number following a prefix of length `prefix_len` in the extension
  // token `ext`. Returns 0 if the suffix contains non-digit characters or is empty.
  auto parse_size = [](std::string_view ext, size_t prefix_len) -> uint16_t {
    std::string_view size_str = ext.substr(prefix_len);
    uint16_t size = 0;
    for (char c : size_str) {
      if (c >= '0' && c <= '9') {
        size = static_cast<uint16_t>(size * 10 + (c - '0'));
      } else {
        return 0;
      }
    }
    return size;
  };

  std::string_view standard_exts = next_token();
  ZX_ASSERT(standard_exts.starts_with("rv32") || standard_exts.starts_with("rv64"));
  standard_exts.remove_prefix(4);  // Eat "rv{32,64}".
  if (standard_exts.find('v') != std::string_view::npos) {
    Set(RiscvFeature::kVector);
  }

  while (!isa_string.empty()) {
    std::string_view ext = next_token();
    if (ext == "sstc"sv) {
      Set(RiscvFeature::kSstc);
    } else if (ext == "svpbmt"sv) {
      Set(RiscvFeature::kSvpbmt);
    } else if (ext == "zicbom"sv) {
      Set(RiscvFeature::kZicbom);
    } else if (ext.starts_with("zicbom"sv)) {
      if (uint16_t size = parse_size(ext, 6); size > 0) {
        Set(RiscvFeature::kZicbom);
        SetCbomSize(size);
      }
    } else if (ext == "zicboz"sv) {
      Set(RiscvFeature::kZicboz);
    } else if (ext.starts_with("zicboz"sv)) {
      if (uint16_t size = parse_size(ext, 6); size > 0) {
        Set(RiscvFeature::kZicboz);
        SetCbozSize(size);
      }
    } else if (ext == "zicntr"sv) {
      Set(RiscvFeature::kZicntr);
    }
  }
  return *this;
}

}  // namespace arch
