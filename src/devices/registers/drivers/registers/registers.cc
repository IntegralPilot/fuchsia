// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "registers.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <string>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/auto_lock.h>

#include "registers_parser.h"

namespace registers {

namespace {

template <typename T>
T GetMask(const fuchsia_hardware_registers::Mask& mask);

template <>
uint8_t GetMask(const fuchsia_hardware_registers::Mask& mask) {
  return static_cast<uint8_t>(mask.r8().value());
}
template <>
uint16_t GetMask(const fuchsia_hardware_registers::Mask& mask) {
  return static_cast<uint16_t>(mask.r16().value());
}
template <>
uint32_t GetMask(const fuchsia_hardware_registers::Mask& mask) {
  return static_cast<uint32_t>(mask.r32().value());
}
template <>
uint64_t GetMask(const fuchsia_hardware_registers::Mask& mask) {
  return static_cast<uint64_t>(mask.r64().value());
}

template <typename T>
zx::result<> CheckOverlappingBits(const fuchsia_hardware_registers::Metadata& metadata,
                                  const std::map<uint32_t, std::shared_ptr<MmioInfo>>& mmios) {
  std::map<uint32_t, std::map<size_t, T>> overlap;
  for (const auto& reg : metadata.registers().value()) {
    if (!reg.name().has_value() || !reg.mmio_id().has_value() || !reg.masks().has_value()) {
      // Doesn't have to have all Register IDs.
      continue;
    }

    auto mmio_id = reg.mmio_id().value();
    if (!mmios.contains(mmio_id)) {
      fdf::error("Invalid MMIO ID {} for Register {}", mmio_id, reg.name().value().c_str());
      return zx::error(ZX_ERR_INTERNAL);
    }

    for (const auto& m : reg.masks().value()) {
      auto mmio_offset = m.mmio_offset().value();
      if (mmio_offset / sizeof(T) >= mmios.at(mmio_id)->locks_.size()) {
        fdf::error("Invalid offset");
        return zx::error(ZX_ERR_INTERNAL);
      }

      if (!m.mask().has_value()) {
        fdf::error("Makse missing mask property");
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      if (!m.overlap_check_on()) {
        continue;
      }

      if (overlap.find(mmio_id) == overlap.end()) {
        overlap[mmio_id] = {};
      }

      for (int i = 0; i < m.count(); i++) {
        auto idx = mmio_offset / sizeof(T) + i;
        if (overlap[mmio_id].find(idx) == overlap[mmio_id].end()) {
          overlap[mmio_id][idx] = 0;
        }

        auto& bits = overlap[mmio_id][idx];
        auto mask = GetMask<T>(m.mask().value());
        if (bits & mask) {
          fdf::error("Overlapping bits in MMIO ID {}, Register No. {}, Bit mask 0x{:x}", mmio_id,
                     idx, static_cast<uint64_t>(bits & mask));
          return zx::error(ZX_ERR_INTERNAL);
        }
        bits |= mask;
      }
    }
  }

  return zx::ok();
}

zx::result<> ValidateMetadata(const fuchsia_hardware_registers::Metadata& metadata,
                              const std::map<uint32_t, std::shared_ptr<MmioInfo>>& mmios) {
  if (!metadata.registers().has_value()) {
    fdf::error("Metadata missing registers field");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  const auto& registers = metadata.registers().value();
  if (registers.empty()) {
    fdf::error("Registers list is empty");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  bool begin = true;
  fuchsia_hardware_registers::Mask::Tag tag = fuchsia_hardware_registers::Mask::Tag::kR32;
  for (size_t i = 0; i < registers.size(); ++i) {
    const auto& reg = registers[i];
    if (!reg.name().has_value()) {
      fdf::error("Register {} missing name field", i);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (!reg.mmio_id().has_value()) {
      fdf::error("Register {} missing mmio_id field", i);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (!reg.masks().has_value()) {
      fdf::error("Register {} missing masks field", i);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    const auto& masks = reg.masks().value();
    if (masks.empty()) {
      fdf::error("Register {} has no masks", i);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    if (begin) {
      if (!masks.begin()->mask().has_value()) {
        fdf::error("First mask of first register missing mask value");
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      tag = masks.begin()->mask().value().Which();
      begin = false;
    }

    for (size_t j = 0; j < masks.size(); ++j) {
      const auto& mask = masks[j];
      if (!mask.mask().has_value()) {
        fdf::error("Mask {} of register {} missing mask field", j, i);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      if (!mask.mmio_offset().has_value()) {
        fdf::error("Mask {} of register {} missing mmio_offset field", j, i);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      if (!mask.count().has_value()) {
        fdf::error("Mask {} of register {} missing count field", j, i);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      if (mask.mask().value().Which() != tag) {
        fdf::error("Width of registers don't match up.");
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      if (mask.mmio_offset().value() % SWITCH_BY_TAG(tag, GetSize)) {
        fdf::error("Mask with offset 0x{:x} is not aligned", mask.mmio_offset().value());
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    }
  }

  return SWITCH_BY_TAG(tag, CheckOverlappingBits, metadata, mmios);
}

}  // namespace

template <typename T>
zx::result<> RegistersDevice::CreateNode(Register<T>& reg) {
  auto result =
      outgoing()->AddService<fuchsia_hardware_registers::Service>(reg.GetHandler(), reg.id());
  if (result.is_error()) {
    fdf::error("Failed to add service to the outgoing directory");
    return result.take_error();
  }

  // Initialize our compat server.
  {
    zx::result result = reg.compat_server_.Initialize(incoming_, outgoing(), node_name_, reg.id());
    if (result.is_error()) {
      return result.take_error();
    }
  }

  fidl::Arena arena;
  auto offers = reg.compat_server_.CreateOffers2(arena);
  offers.push_back(fdf::MakeOffer2<fuchsia_hardware_registers::Service>(arena, reg.id()));
  auto properties = std::vector{
      fdf::MakeProperty2(arena, bind_fuchsia::NAME, reg.id()),
      fdf::MakeProperty2(arena, bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
  };
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, "register-" + reg.id())
                  .offers2(arena, std::move(offers))
                  .properties2(arena, std::move(properties))
                  .Build();

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  {
    fidl::WireResult result =
        fidl::WireCall(node())->AddChild(args, std::move(controller_endpoints.server), {});
    if (!result.ok()) {
      fdf::error("Failed to add child {}", result.FormatDescription().c_str());
      return zx::error(result.status());
    }
  }
  reg.controller_.Bind(std::move(controller_endpoints.client));

  return zx::ok();
}

template <typename T>
zx::result<> RegistersDevice::Create(
    const fuchsia_hardware_registers::RegistersMetadataEntry& reg) {
  if (!reg.name().has_value()) {
    fdf::error("Register missing name field");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!reg.mmio_id().has_value()) {
    fdf::error("Register missing mmio_id field");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!reg.masks().has_value()) {
    fdf::error("Register missing masks field");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::map<uint64_t, std::pair<T, uint32_t>> masks;
  for (const auto& m : reg.masks().value()) {
    auto mask = GetMask<T>(m.mask().value());
    masks.emplace(m.mmio_offset().value(), std::make_pair(mask, m.count().value()));
  }
  return std::visit(
      [&](auto&& d) { return CreateNode(d); },
      registers_.emplace_back(std::in_place_type<Register<T>>, mmios_[reg.mmio_id().value()],
                              std::string(reg.name().value()), std::move(masks)));
}

zx::result<> RegistersDevice::MapMmio(fuchsia_hardware_registers::Mask::Tag& tag) {
  zx::result result = incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (result.is_error()) {
    fdf::error("Failed to open pdev service: {}", result);
    return result.take_error();
  }
  fidl::WireSyncClient pdev(std::move(result.value()));
  if (!pdev.is_valid()) {
    fdf::error("Failed to get pdev");
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  auto device_info = pdev->GetNodeDeviceInfo();
  if (!device_info.ok() || device_info->is_error()) {
    fdf::error("Could not get device info {}", device_info.FormatDescription().c_str());
    return zx::error(device_info.ok() ? device_info->error_value() : device_info.error().status());
  }

  ZX_ASSERT(device_info->value()->has_mmio_count());
  for (uint32_t i = 0; i < device_info->value()->mmio_count(); i++) {
    auto mmio = pdev->GetMmioById(i);
    if (!mmio.ok() || mmio->is_error()) {
      fdf::error("Could not get mmio regions {}", mmio.FormatDescription().c_str());
      return zx::error(mmio.ok() ? mmio->error_value() : mmio.error().status());
    }

    if (!mmio->value()->has_vmo() || !mmio->value()->has_size() || !mmio->value()->has_offset()) {
      fdf::error("GetMmioById({}) returned invalid MMIO", i);
      return zx::error(ZX_ERR_BAD_STATE);
    }

    zx::result mmio_buffer =
        fdf::MmioBuffer::Create(mmio->value()->offset(), mmio->value()->size(),
                                std::move(mmio->value()->vmo()), ZX_CACHE_POLICY_UNCACHED_DEVICE);
    if (mmio_buffer.is_error()) {
      fdf::error("Failed to map MMIO: {}", mmio_buffer);
      return zx::error(mmio_buffer.error_value());
    }

    zx::result<MmioInfo> mmio_info = SWITCH_BY_TAG(tag, MmioInfo::Create, std::move(*mmio_buffer));
    if (mmio_info.is_error()) {
      fdf::error("Could not create mmio info {}", mmio_info.error_value());
      return zx::error(mmio_info.take_error());
    }

    mmios_.emplace(i, std::make_shared<MmioInfo>(std::move(*mmio_info)));
  }

  return zx::ok();
}

static zx::result<fuchsia_hardware_registers::Metadata> ConvertMetadata(
    const registers_metadata::RegistersMetadata& generic) {
  fuchsia_hardware_registers::Metadata metadata;
  std::vector<fuchsia_hardware_registers::RegistersMetadataEntry> registers;

  for (const auto& r : generic.registers) {
    fuchsia_hardware_registers::RegistersMetadataEntry entry;
    entry.name(r.name);
    entry.mmio_id(r.mmio_id);

    std::vector<fuchsia_hardware_registers::MaskEntry> masks;
    for (const auto& m : r.masks) {
      fuchsia_hardware_registers::MaskEntry mask_entry;
      mask_entry.mmio_offset(m.offset);
      mask_entry.count(m.count.value_or(1));
      if (m.overlap_check) {
        mask_entry.overlap_check_on(*m.overlap_check);
      }

      zx::result<fuchsia_hardware_registers::Mask> fidl_mask_res =
          [&]() -> zx::result<fuchsia_hardware_registers::Mask> {
        switch (m.size) {
          case 1:
            return zx::ok(fuchsia_hardware_registers::Mask::WithR8(static_cast<uint8_t>(m.mask)));
          case 2:
            return zx::ok(fuchsia_hardware_registers::Mask::WithR16(static_cast<uint16_t>(m.mask)));
          case 4:
            return zx::ok(fuchsia_hardware_registers::Mask::WithR32(static_cast<uint32_t>(m.mask)));
          case 8:
            return zx::ok(fuchsia_hardware_registers::Mask::WithR64(m.mask));
          default:
            fdf::error("Unsupported register size: {}", m.size);
            return zx::error(ZX_ERR_INVALID_ARGS);
        }
      }();
      if (fidl_mask_res.is_error()) {
        return fidl_mask_res.take_error();
      }
      mask_entry.mask(std::move(fidl_mask_res.value()));
      masks.push_back(std::move(mask_entry));
    }
    entry.masks(std::move(masks));
    registers.push_back(std::move(entry));
  }

  metadata.registers(std::move(registers));
  return zx::ok(metadata);
}

zx::result<> RegistersDevice::Start(fdf::DriverContext context) {
  node_name_ = context.node_name().value_or("");
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  // Get metadata.
  zx::result pdev_client =
      incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client.value())};

  fuchsia_hardware_registers::Metadata metadata;
  {
    // Try to get generic metadata first
    zx::result generic_res = pdev.GetFidlMetadata<fuchsia_driver_metadata::Dictionary>(
        "fuchsia.hardware.registers.Metadata");
    if (generic_res.is_ok()) {
      auto parsed = registers_metadata::RegistersMetadata::Parse(generic_res.value());
      if (parsed) {
        auto converted = ConvertMetadata(*parsed);
        if (converted.is_ok()) {
          metadata = std::move(converted.value());
        } else {
          fdf::error("Failed to convert generic Registers metadata: {}", converted.status_string());
          return converted.take_error();
        }
      } else {
        fdf::error("Failed to parse generic Registers metadata");
        return zx::error(ZX_ERR_INTERNAL);
      }
    }

    if (!metadata.registers().has_value()) {
      // Fall back to old metadata
      zx::result metadata_result = pdev.GetFidlMetadata<fuchsia_hardware_registers::Metadata>();
      if (metadata_result.is_error()) {
        fdf::error("Failed to get metadata: {}", metadata_result);
        return metadata_result.take_error();
      }
      metadata = std::move(metadata_result.value());
    }
  }

  if (!metadata.registers().has_value() || metadata.registers().value().empty()) {
    fdf::error("Metadata missing registers or registers list is empty");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!metadata.registers().value()[0].masks().has_value() ||
      metadata.registers().value()[0].masks().value().empty()) {
    fdf::error("First register missing masks or masks list is empty");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!metadata.registers().value()[0].masks().value()[0].mask().has_value()) {
    fdf::error("First mask missing mask value");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto tag = metadata.registers().value()[0].masks().value()[0].mask().value().Which();

  // Get mmio.
  {
    auto result = MapMmio(tag);
    if (result.is_error()) {
      fdf::error("Failed to map MMIOs: {}", result);
      return result.take_error();
    }
  }

  // Validate metadata.
  {
    auto result = ValidateMetadata(metadata, mmios_);
    if (result.is_error()) {
      fdf::error("Failed to validate metadata: {}", result);
      return result.take_error();
    }
  }

  // Create devices.
  for (const auto& reg : metadata.registers().value()) {
    auto result = SWITCH_BY_TAG(tag, Create, reg);
    if (result.is_error()) {
      fdf::error("Failed to create device for {}: {}", reg.name().value().c_str(),
                 result.status_string());
    }
  }

  return zx::ok();
}

}  // namespace registers

FUCHSIA_DRIVER_EXPORT2(registers::RegistersDevice);
