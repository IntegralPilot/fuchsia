// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl-metadata/registers.h"

#include <fidl/fuchsia.hardware.registers/cpp/wire.h>

#include <zxtest/zxtest.h>

template <typename T>
static void check_encodes(
    const cpp20::span<const fidl_metadata::registers::Register<T>> register_entries) {
  // Encode.
  zx::result metadata_result = fidl_metadata::registers::RegistersMetadataToFidl(register_entries);
  ASSERT_OK(metadata_result.status_value());
  const auto& metadata = metadata_result.value();

  // Check everything looks sensible.
  ASSERT_TRUE(metadata.registers().has_value());
  const auto& registers = metadata.registers().value();
  ASSERT_EQ(registers.size(), register_entries.size());

  for (size_t i = 0; i < register_entries.size(); i++) {
    const auto& reg = registers[i];
    ASSERT_EQ(reg.name(), register_entries[i].name);

    ASSERT_EQ(reg.mmio_id(), register_entries[i].mmio_id);

    ASSERT_TRUE(reg.masks().has_value());
    ASSERT_EQ(reg.masks().value().size(), register_entries[i].masks.size());
    for (size_t j = 0; j < register_entries[i].masks.size(); j++) {
      const auto& mask_entry = reg.masks().value()[j];
      ASSERT_TRUE(mask_entry.mask().has_value());
      const auto& mask = mask_entry.mask().value();
      if constexpr (std::is_same_v<T, uint8_t>) {
        ASSERT_EQ(mask.Which(), fuchsia_hardware_registers::Mask::Tag::kR8);
        ASSERT_EQ(mask.r8().value(), register_entries[i].masks[j].value);
      } else if constexpr (std::is_same_v<T, uint16_t>) {
        ASSERT_EQ(mask.Which(), fuchsia_hardware_registers::Mask::Tag::kR16);
        ASSERT_EQ(mask.r16().value(), register_entries[i].masks[j].value);
      } else if constexpr (std::is_same_v<T, uint32_t>) {
        ASSERT_EQ(mask.Which(), fuchsia_hardware_registers::Mask::Tag::kR32);
        ASSERT_EQ(mask.r32().value(), register_entries[i].masks[j].value);
      } else if constexpr (std::is_same_v<T, uint64_t>) {
        ASSERT_EQ(mask.Which(), fuchsia_hardware_registers::Mask::Tag::kR64);
        ASSERT_EQ(mask.r64().value(), register_entries[i].masks[j].value);
      } else {
        ASSERT_TRUE(false);
      }

      ASSERT_EQ(mask_entry.mmio_offset(), register_entries[i].masks[j].mmio_offset);

      ASSERT_EQ(mask_entry.count(), register_entries[i].masks[j].count);

      ASSERT_EQ(mask_entry.overlap_check_on(), register_entries[i].masks[j].overlap_check_on);
    }
  }
}

TEST(RegistersMetadataTest, TestEncode8) {
  static const fidl_metadata::registers::Register<uint8_t> kRegisters[]{
      {
          .name = "test",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0x98,
                      .mmio_offset = 0x20,
                  },
              },
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes<uint8_t>(kRegisters));
}

TEST(RegistersMetadataTest, TestEncode16) {
  static const fidl_metadata::registers::Register<uint16_t> kRegisters[]{
      {
          .name = "test",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xBEEF,
                      .mmio_offset = 0x20,
                  },
              },
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes<uint16_t>(kRegisters));
}

TEST(RegistersMetadataTest, TestEncode32) {
  static const fidl_metadata::registers::Register<uint32_t> kRegisters[]{
      {
          .name = "test",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xFFFF0000,
                      .mmio_offset = 0x20,
                  },
              },
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes<uint32_t>(kRegisters));
}

TEST(RegistersMetadataTest, TestEncode64) {
  static const fidl_metadata::registers::Register<uint64_t> kRegisters[]{
      {
          .name = "test",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0x012345678ABCDEF0,
                      .mmio_offset = 0x20,
                  },
              },
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes<uint64_t>(kRegisters));
}

TEST(RegistersMetadataTest, TestEncodeCountAndOverlapCheck) {
  static const fidl_metadata::registers::Register<uint32_t> kRegisters[]{
      {
          .name = "test",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xFFFF0000,
                      .mmio_offset = 0x20,
                      .count = 8,
                      .overlap_check_on = false,
                  },
              },
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes<uint32_t>(kRegisters));
}

TEST(RegistersMetadataTest, TestEncodeMany) {
  static const fidl_metadata::registers::Register<uint16_t> kRegisters[]{
      {
          .name = "test0",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xFFFF,
                      .mmio_offset = 0x20,
                      .count = 5,
                      .overlap_check_on = false,
                  },
              },
      },
      {
          .name = "test1",
          .mmio_id = 4,
          .masks =
              {
                  {
                      .value = 0xABCD,
                      .mmio_offset = 0x7,
                  },
                  {
                      .value = 0x8765,
                      .mmio_offset = 0x0,
                      .count = 2,
                  },
              },
      },
  };

  ASSERT_NO_FATAL_FAILURE(check_encodes<uint16_t>(kRegisters));
}

TEST(RegistersMetadataTest, TestEncodeNoRegisters) {
  ASSERT_NO_FATAL_FAILURE(
      check_encodes(cpp20::span<const fidl_metadata::registers::Register<uint32_t>>()));
}

TEST(RegistersMetadataTest, TestEncodeLongNameFails) {
  static const fidl_metadata::registers::Register<uint32_t> kRegisters[]{
      {
          .name = "test-with-a-really-long-register-name",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xFFFF0000,
                      .mmio_offset = 0x20,
                      .count = 8,
                      .overlap_check_on = false,
                  },
              },
      },
  };

  auto result = fidl_metadata::registers::RegistersMetadataToFidl(kRegisters);
  ASSERT_TRUE(result.is_error());
}
