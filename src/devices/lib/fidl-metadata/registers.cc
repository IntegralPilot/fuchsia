// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl-metadata/registers.h"

namespace fidl_metadata::registers {

namespace {

template <typename T>
zx::result<fuchsia_hardware_registers::Metadata> ToFidl(cpp20::span<const Register<T>> registers) {
  std::vector<fuchsia_hardware_registers::RegistersMetadataEntry> registers_metadata;
  registers_metadata.reserve(registers.size());

  for (const auto& src_reg : registers) {
    if (src_reg.name.size() > fuchsia_hardware_registers::kMaxNameLength) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    std::vector<fuchsia_hardware_registers::MaskEntry> mask_entries;
    mask_entries.reserve(src_reg.masks.size());
    for (const auto& src_mask : src_reg.masks) {
      fuchsia_hardware_registers::MaskEntry entry{{
          .mmio_offset{src_mask.mmio_offset},
          .count{src_mask.count},
          .overlap_check_on{src_mask.overlap_check_on},
      }};

      if constexpr (std::is_same_v<T, uint8_t>) {
        entry.mask().emplace(fuchsia_hardware_registers::Mask::WithR8(src_mask.value));
      } else if constexpr (std::is_same_v<T, uint16_t>) {
        entry.mask().emplace(fuchsia_hardware_registers::Mask::WithR16(src_mask.value));
      } else if constexpr (std::is_same_v<T, uint32_t>) {
        entry.mask().emplace(fuchsia_hardware_registers::Mask::WithR32(src_mask.value));
      } else if constexpr (std::is_same_v<T, uint64_t>) {
        entry.mask().emplace(fuchsia_hardware_registers::Mask::WithR64(src_mask.value));
      } else {
        static_assert(false);
      }

      mask_entries.emplace_back(std::move(entry));
    }

    registers_metadata.emplace_back(fuchsia_hardware_registers::RegistersMetadataEntry{{
        .name{src_reg.name},
        .mmio_id{src_reg.mmio_id},
        .masks{std::move(mask_entries)},
    }});
  }

  return zx::ok(fuchsia_hardware_registers::Metadata{{.registers{std::move(registers_metadata)}}});
}

}  // namespace

zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register8> registers) {
  return ToFidl(registers);
}
zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register16> registers) {
  return ToFidl(registers);
}
zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register32> registers) {
  return ToFidl(registers);
}
zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register64> registers) {
  return ToFidl(registers);
}

}  // namespace fidl_metadata::registers
