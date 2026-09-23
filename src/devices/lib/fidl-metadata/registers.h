// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_FIDL_METADATA_REGISTERS_H_
#define SRC_DEVICES_LIB_FIDL_METADATA_REGISTERS_H_

#include <fidl/fuchsia.hardware.registers/cpp/fidl.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/result.h>
#include <stdint.h>

#include <string>
#include <vector>

namespace fidl_metadata::registers {

// Structs match fuchsia.hardware.registers/metadata.fidl
template <typename T>
struct Mask {
  T value;
  uint64_t mmio_offset;
  uint32_t count = 1;
  bool overlap_check_on = true;
};

template <typename T>
struct Register {
  std::string name;
  uint32_t mmio_id;
  std::vector<Mask<T>> masks;
};

using Register8 = Register<uint8_t>;
using Register16 = Register<uint16_t>;
using Register32 = Register<uint32_t>;
using Register64 = Register<uint64_t>;

zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register8> registers);
zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register16> registers);
zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register32> registers);
zx::result<fuchsia_hardware_registers::Metadata> RegistersMetadataToFidl(
    cpp20::span<const Register64> registers);

}  // namespace fidl_metadata::registers

#endif  // SRC_DEVICES_LIB_FIDL_METADATA_REGISTERS_H_
