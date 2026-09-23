// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bus/drivers/platform/platform-device.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/zbi-format/partition.h>
#include <lib/zircon-internal/align.h>
#include <zircon/errors.h>
#include <zircon/syscalls/resource.h>
#include <zircon/system/public/zircon/syscalls-next.h>

#include <algorithm>
#include <unordered_set>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/resource/cpp/bind.h>
#include <fbl/algorithm.h>

#include "src/devices/bus/drivers/platform/node-util.h"
#include "src/devices/bus/drivers/platform/platform-bus.h"
#include "src/devices/bus/drivers/platform/platform-interrupt.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace {

fuchsia_boot_metadata::SerialNumberMetadata CreateSerialNumberMetadata(
    std::span<const uint8_t> bytes) {
  std::string serial_number{bytes.begin(), bytes.end()};
  return fuchsia_boot_metadata::SerialNumberMetadata{{.serial_number{std::move(serial_number)}}};
}

zx::result<fuchsia_boot_metadata::PartitionMap> CreatePartitionMap(std::span<const uint8_t> bytes) {
  if (bytes.size() < sizeof(zbi_partition_map_t)) {
    fdf::error("Incorrect number of bytes: Expected at least {} bytes but actual is {} bytes",
               sizeof(zbi_partition_map_t), bytes.size());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  const auto& src_map = *reinterpret_cast<const zbi_partition_map_t*>(bytes.data());
  fuchsia_boot_metadata::PartitionMap dst_map{{.block_count = src_map.block_count,
                                               .block_size = src_map.block_size,
                                               .reserved = src_map.reserved,
                                               .guid{{0}}}};
  std::ranges::copy(src_map.guid, dst_map.guid().value().begin());

  auto partitions_byte_count = (bytes.size() - sizeof(zbi_partition_map_t));
  auto expected_partitions_byte_count = src_map.partition_count * sizeof(zbi_partition_t);
  if (partitions_byte_count != expected_partitions_byte_count) {
    fdf::error("Incorrect number of bytes: Expected {} bytes but actual is {} bytes",
               sizeof(zbi_partition_map_t) + expected_partitions_byte_count,
               sizeof(zbi_partition_map_t) + partitions_byte_count);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  const auto* first_partition =
      reinterpret_cast<const zbi_partition_t*>(bytes.data() + sizeof(zbi_partition_map_t));
  std::span<const zbi_partition_t> src_partitions{first_partition, src_map.partition_count};

  auto dst_partitions = src_partitions | std::views::transform([](const zbi_partition_t& src) {
                          fuchsia_boot_metadata::Partition dst{{.type_guid{{0}},
                                                                .unique_guid{{0}},
                                                                .first_block = src.first_block,
                                                                .last_block = src.last_block,
                                                                .flags = src.flags,
                                                                .name{src.name}}};
                          std::ranges::copy(src.type_guid, dst.type_guid().begin());
                          std::ranges::copy(src.uniq_guid, dst.unique_guid().begin());
                          return dst;
                        });
  dst_map.partitions().emplace(dst_partitions.begin(), dst_partitions.end());

  return zx::ok(std::move(dst_map));
}

zx::result<fuchsia_boot_metadata::MacAddressMetadata> CreateMacAddressMetadata(
    std::span<const uint8_t> bytes) {
  fuchsia_net::MacAddress mac_address;
  if (bytes.size() != mac_address.octets().size()) {
    fdf::error("Size of encoded MAC address is incorrect: expected {} bytes but actual is {} bytes",
               mac_address.octets().size(), bytes.size());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  std::ranges::copy(bytes.begin(), bytes.end(), mac_address.octets().begin());
  return zx::ok(fuchsia_boot_metadata::MacAddressMetadata{{.mac_address = std::move(mac_address)}});
}

}  // namespace

namespace platform_bus {

namespace fpbus = fuchsia_hardware_platform_bus;

zx::result<std::unique_ptr<PlatformDevice>> PlatformDevice::Create(
    fpbus::Node node, PlatformBus* bus, inspect::ComponentInspector& inspector) {
  auto inspect_node_name = node.name().value() + "-platform-device";
  auto dev = std::make_unique<platform_bus::PlatformDevice>(
      bus, inspector.root().CreateChild(inspect_node_name), std::move(node));
  zx::result result = dev->Init();
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::move(dev));
}

fpromise::promise<inspect::Inspector> PlatformDevice::InspectNodeCallback() const {
  inspect::Inspector inspector;
  auto interrupt_vectors =
      inspector.GetRoot().CreateUintArray("interrupt_vectors", interrupt_vectors_.size());
  size_t i = 0;
  for (const auto& vector : interrupt_vectors_) {
    interrupt_vectors.Set(i++, vector);
  }
  inspector.emplace(std::move(interrupt_vectors));
  return fpromise::make_result_promise(fpromise::ok(std::move(inspector)));
}

PlatformDevice::PlatformDevice(PlatformBus* bus, inspect::Node inspect_node, fpbus::Node node)
    : bus_(bus),
      name_(node.name().value()),
      vid_(node.vid().value_or(0)),
      pid_(node.pid().value_or(0)),
      did_(node.did().value_or(0)),
      instance_id_(node.instance_id().value_or(0)),
      node_(std::move(node)),
      inspect_node_(std::move(inspect_node)) {}

zx::result<PlatformDevice::Mmio> PlatformDevice::GetMmio(uint32_t index) const {
  if (node_.mmio() == std::nullopt || index >= node_.mmio()->size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const auto& mmio = node_.mmio().value()[index];
  if (unlikely(!IsValid(mmio))) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (mmio.base() == std::nullopt) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const zx_paddr_t vmo_base = fbl::round_down(mmio.base().value(), zx_system_get_page_size());
  const size_t vmo_size = fbl::round_up(mmio.base().value() + mmio.length().value() - vmo_base,
                                        zx_system_get_page_size());
  zx::vmo vmo;

  zx_status_t status = zx::vmo::create_physical(*bus_->GetMmioResource(), vmo_base, vmo_size, &vmo);
  if (status != ZX_OK) {
    fdf::error("creating vmo failed {}", zx_status_get_string(status));
    return zx::error(status);
  }

  char name[32]{};
  std::format_to_n(name, sizeof(name) - 1, "mmio {}", index);
  status = vmo.set_property(ZX_PROP_NAME, name, sizeof(name));
  if (status != ZX_OK) {
    fdf::error("setting vmo name failed {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(Mmio{
      .offset = mmio.base().value() - vmo_base,
      .size = mmio.length().value(),
      .vmo = vmo.release(),
  });
}

void PlatformDevice::GetInterrupt(uint32_t index, uint32_t flags, GetInterruptCallback callback) {
  if (node_.irq() == std::nullopt || index >= node_.irq()->size()) {
    callback(zx::error(ZX_ERR_OUT_OF_RANGE));
    return;
  }

  const auto& irq = node_.irq().value()[index];
  if (unlikely(!IsValid(irq))) {
    callback(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  const fuchsia_hardware_platform_bus::IrqSpec& irq_spec = irq.irq().value();
  const bool is_kernel_interrupt = irq_spec.irq().has_value();

  uint32_t vector;
  if (is_kernel_interrupt) {
    vector = irq_spec.irq().value();
  } else if (irq_spec.userspace_irq().has_value()) {
    vector = irq_spec.userspace_irq()->irq();
  } else {
    fdf::error("IRQ type is neither kernel nor userspace");
    callback(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  // If the driver chose "default" for the IRQ mode, use the configuration we have instead.
  const uint32_t cfg_mode = static_cast<uint32_t>(irq.mode().value()) & ZX_INTERRUPT_MODE_MASK;
  const uint32_t drv_mode = flags & ZX_INTERRUPT_MODE_MASK;

  if (drv_mode == ZX_INTERRUPT_MODE_DEFAULT) {
    fdf::info(
        "IRQ vector {} for platform device \"{}\" has default mode {:#08x}.  "
        "Using config mode {:08x} instead.",
        vector, name_, drv_mode, cfg_mode);
    flags = (flags & ~ZX_INTERRUPT_MODE_MASK) | cfg_mode;
  } else {
    // If the driver explicitly passed in a mode value which conflicts with our
    // mode value, print a warning, but use their value anyway.
    if (drv_mode != cfg_mode) {
      fdf::warn(
          "IRQ vector {} for platform device \"{}\" explicitly requesting mode {:#08x} which"
          " does not match platform configuration {:#08x}",
          vector, name_, drv_mode, cfg_mode);
    }
  }

  if (flags & ZX_INTERRUPT_WAKE_VECTOR) {
    fdf::error("Client passing in ZX_INTERRUPT_WAKE_VECTOR. Not allowed");
    callback(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  if (bus_->suspend_enabled() && irq.wake_vector().has_value() && irq.wake_vector().value()) {
    flags |= ZX_INTERRUPT_WAKE_VECTOR;
  }
  fdf::info("Creating interrupt with vector {} (flags {:#08x}) for platform device \"{}\"", vector,
            flags, name_);
  zx::interrupt out_irq;

  zx_status_t status;
  if (is_kernel_interrupt) {
    status = zx::interrupt::create(*bus_->GetIrqResource(), vector, flags, &out_irq);
  } else {
    status = zx::interrupt::create(
        zx::resource(), 0, ZX_INTERRUPT_VIRTUAL | (flags & ZX_INTERRUPT_TIMESTAMP_MONO), &out_irq);
  }
  if (status != ZX_OK) {
    fdf::error("zx_interrupt_create failed {}", zx_status_get_string(status));
    callback(zx::error(status));
    return;
  }
  interrupt_vectors_.insert(vector);

  zx_info_handle_basic_t info;
  status = out_irq.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ZX_ASSERT(status == ZX_OK);
  interrupt_koids_.insert(info.koid);

  if (is_kernel_interrupt) {
    callback(zx::ok(std::move(out_irq)));
  } else {
    bus_->RegisterInterrupt(irq_spec.userspace_irq().value(), flags, std::move(out_irq),
                            std::move(callback));
  }
}

zx::result<zx::bti> PlatformDevice::GetBti(uint32_t index) const {
  if (node_.bti() == std::nullopt || index >= node_.bti()->size()) {
    fdf::error("Trying to get bti with index {}", index);
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const auto& bti = node_.bti().value()[index];
  if (unlikely(!IsValid(bti))) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  return bus_->GetBti(bti.iommu_id().value(), bti.bti_id().value(), name());
}

zx::result<zx::resource> PlatformDevice::GetSmc(uint32_t index) const {
  if (node_.smc() == std::nullopt || index >= node_.smc()->size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const auto& smc = node_.smc().value()[index];
  if (unlikely(!IsValid(smc))) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  uint32_t options = ZX_RSRC_KIND_SMC;
  if (smc.exclusive().value()) {
    options |= ZX_RSRC_FLAG_EXCLUSIVE;
  }
  char rsrc_name[ZX_MAX_NAME_LEN] = {};
  std::format_to_n(rsrc_name, sizeof(rsrc_name) - 1, "{}.pbus[{}]", name_, index);
  zx::resource out_resource;
  zx_status_t status =
      zx::resource::create(*bus_->GetSmcResource(), options, smc.service_call_num_base().value(),
                           smc.count().value(), rsrc_name, sizeof(rsrc_name), &out_resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(out_resource));
}

PlatformDevice::DeviceInfo PlatformDevice::GetDeviceInfo() const {
  return DeviceInfo{
      .vid = vid_,
      .pid = pid_,
      .did = did_,
      .mmio_count = static_cast<uint32_t>(node_.mmio().has_value() ? node_.mmio()->size() : 0),
      .irq_count = static_cast<uint32_t>(node_.irq().has_value() ? node_.irq()->size() : 0),
      .bti_count = static_cast<uint32_t>(node_.bti().has_value() ? node_.bti()->size() : 0),
      .smc_count = static_cast<uint32_t>(node_.smc().has_value() ? node_.smc()->size() : 0),
      .metadata_count =
          static_cast<uint32_t>(node_.metadata().has_value() ? node_.metadata()->size() : 0),
      .reserved = {},
      .name = name_,
  };
}

PlatformDevice::BoardInfo PlatformDevice::GetBoardInfo() const {
  auto info = bus_->board_info();
  return BoardInfo{
      .vid = info.vid(),
      .pid = info.pid(),
      .board_revision = info.board_revision(),
      .board_name = info.board_name(),
  };
}

zx::result<> PlatformDevice::CreateNode() {
  std::optional<fdf::DeviceAddress> address;
  auto bus_type = fdf::BusType::kPlatform;

  std::string name;
  if (did_ == PDEV_DID_DEVICETREE_NODE) {
    name = name_;
    bus_type = fdf::BusType::kDeviceTree;
    address = fdf::DeviceAddress::WithStringValue(name_);
  } else {
    name = name_;
    address = fdf::DeviceAddress::WithStringValue(name_);
  }

  auto bus_info = fdf::BusInfo{{
      .bus = bus_type,
      .address = address,
      .address_stability = fdf::DeviceAddressStability::kStable,
  }};

  std::vector props{
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID, vid_),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID, pid_),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID, did_),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, instance_id_),
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_platform::BIND_PROTOCOL_DEVICE),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.device.Service"),
  };
  if (const auto& node_props = node_.properties(); node_props.has_value()) {
    std::copy(node_props->cbegin(), node_props->cend(), std::back_inserter(props));
  }

  auto add_props = [&props](const auto& resource, const std::string& count_key,
                            const char* resource_key_prefix) {
    const uint32_t count = resource.has_value() ? static_cast<uint32_t>(resource->size()) : 0u;
    props.emplace_back(fdf::MakeProperty2(count_key, count));

    for (uint32_t i = 0; i < count; i++) {
      const auto& name = resource.value()[i].name();
      const std::string key = resource_key_prefix + std::to_string(i);
      const std::string value = name.has_value() ? name.value() : "unknown";
      props.emplace_back(fdf::MakeProperty2(key, value));
    }
  };
  add_props(node_.mmio(), bind_fuchsia_resource::MMIO_COUNT, "fuchsia.resource.MMIO_");
  add_props(node_.irq(), bind_fuchsia_resource::INTERRUPT_COUNT, "fuchsia.resource.INTERRUPT_");
  add_props(node_.bti(), bind_fuchsia_resource::BTI_COUNT, "fuchsia.resource.BTI_");
  add_props(node_.smc(), bind_fuchsia_resource::SMC_COUNT, "fuchsia.resource.SMC_");

  std::vector offers = {
      fdf::MakeOffer2<fuchsia_hardware_platform_device::Service>(name_),
  };
  if (node_.interrupt_controller_id().has_value()) {
    offers.push_back(fdf::MakeOffer2<fuchsia_hardware_interrupt::ControllerRegistryService>(name_));
  }
  if (device_server_) {
    offers.push_back(fdf::MakeOffer2<fuchsia_driver_compat::Service>(name_));
  }
  if (serial_number_metadata_server_) {
    std::optional serial_number_offer = serial_number_metadata_server_->CreateOffer();
    if (serial_number_offer.has_value()) {
      offers.push_back(std::move(serial_number_offer.value()));
    }
  }
  if (partition_map_metadata_server_) {
    std::optional partition_map_offer = partition_map_metadata_server_->CreateOffer();
    if (partition_map_offer.has_value()) {
      offers.push_back(std::move(partition_map_offer.value()));
    }
  }
  if (mac_address_metadata_server_) {
    std::optional mac_address_offer = mac_address_metadata_server_->CreateOffer();
    if (mac_address_offer.has_value()) {
      offers.push_back(std::move(mac_address_offer.value()));
    }
  }

  // Add our offers to the outgoing directory
  {
    zx::result result = bus()->outgoing()->AddService<fuchsia_hardware_platform_device::Service>(
        fuchsia_hardware_platform_device::Service::InstanceHandler({
            .device = device_bindings_.CreateHandler(
                this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                fidl::kIgnoreBindingClosure),
        }),
        name_);
    if (result.is_error()) {
      fdf::warn("Failed to add platform device service: {}", result);
      return result.take_error();
    }
  }

  if (node_.interrupt_controller_id().has_value()) {
    zx::result result =
        bus()->outgoing()->AddService<fuchsia_hardware_interrupt::ControllerRegistryService>(
            fuchsia_hardware_interrupt::ControllerRegistryService::InstanceHandler({
                .registry = controller_bindings_.CreateHandler(
                    this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                    fidl::kIgnoreBindingClosure),
            }),
            name_);
    if (result.is_error()) {
      fdf::warn("Failed to add controller registry service: {}", result);
      return result.take_error();
    }
  }

  auto [client, server] = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  fuchsia_driver_framework::NodeAddArgs args{{
      .name = {name},
      .offers2 = std::move(offers),
      .bus_info = std::move(bus_info),
      .properties2 = std::move(props),
      .driver_host = node_.driver_host(),
  }};

  auto parent = bus()->platform_node();
  fidl::Result result = fidl::Call(parent)->AddChild({std::move(args), std::move(server), {}});

  if (result.is_error()) {
    fdf::error("Failed to add child {}. Error: {}", name, result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  node_controller_.Bind(std::move(client), bus()->dispatcher());
  node_controller_->WaitForDriver().Then(
      [this](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_error()) {
          fdf::warn("Failed to wait for driver start for {}: {}", name_,
                    result.error_value().FormatDescription());
          return;
        }

        switch (result.value().Which()) {
          case fuchsia_driver_framework::DriverResult::Tag::kDriverStartedNodeToken: {
            fdf::debug("platform device {} had driver start.", name_);
            node_token_ = result.value().driver_started_node_token().take();
            break;
          }
          case fuchsia_driver_framework::DriverResult::Tag::kMatchError: {
            fdf::info("platform device {} did not match a driver/composite.", name_);
            break;
          }
          case fuchsia_driver_framework::DriverResult::Tag::kStartError: {
            fdf::warn("platform device {} failed to start.", name_);
            break;
          }
          default: {
            fdf::error("platform device {} unrecognized driver result type.", name_);
          }
        }
      });

  return zx::ok();
}

zx::result<> PlatformDevice::Init() {
  if (node_.irq().has_value()) {
    for (uint32_t i = 0; i < node_.irq()->size(); i++) {
      auto fragment = std::make_unique<PlatformInterruptFragment>(
          this, i, fdf::Dispatcher::GetCurrent()->async_dispatcher());
      auto name = std::format("{}-irq{:03}", name_, i);
      zx::result result = fragment->Add(name.c_str(), this, node_.irq().value()[i]);
      if (result.is_error()) {
        fdf::warn("Failed to create interrupt fragment {}", i);
        continue;
      }

      fragments_.push_back(std::move(fragment));
    }
  }

  const size_t metadata_count = node_.metadata() == std::nullopt ? 0 : node_.metadata()->size();
  const size_t boot_metadata_count =
      node_.boot_metadata() == std::nullopt ? 0 : node_.boot_metadata()->size();
  if (metadata_count > 0 || boot_metadata_count > 0) {
    device_server_ = std::make_unique<compat::DeviceServer>();
    device_server_->Initialize(name_);
    device_server_->Serve(bus()->dispatcher(), bus()->outgoing().get());
  }

  inspect_node_.RecordLazyValues("interrupt_vectors",
                                 fit::bind_member<&PlatformDevice::InspectNodeCallback>(this));

  for (size_t i = 0; i < metadata_count; i++) {
    const auto& metadata = node_.metadata().value()[i];
    if (!IsValid(metadata)) {
      fdf::info("Metadata at index {} is invalid", i);
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto metadata_id = metadata.id();
    ZX_ASSERT(metadata_id.has_value());
    auto metadata_data = metadata.data();
    ZX_ASSERT(metadata_data.has_value());

    // TODO(b/341981272): Remove `device_server.AddMetadata()` once all drivers bound to platform
    // devices do not use `device_get_metadata()` to retrieve metadata. They should be using
    // fuchsia.hardware.platform.device/Device::GetMetadata().
    errno = 0;
    char* metadata_id_end{};
    const char* metadata_id_start = metadata_id.value().c_str();
    auto metadata_type =
        static_cast<uint32_t>(std::strtol(metadata_id_start, &metadata_id_end, 10));
    if (!metadata_id.value().empty() && errno == 0 && *metadata_id_end == '\0') {
      zx_status_t status =
          device_server_->AddMetadata(metadata_type, metadata_data->data(), metadata_data->size());
      if (status != ZX_OK) {
        fdf::info("Failed to add metadata with ID {}: {}", metadata_id.value().c_str(),
                  zx_status_get_string(status));
        return zx::error(status);
      }
    }

    metadata_.emplace(metadata_id.value(), metadata_data.value());
  }

  for (size_t i = 0; i < boot_metadata_count; i++) {
    const auto& metadata = node_.boot_metadata().value()[i];
    if (!IsValid(metadata)) {
      fdf::info("Boot metadata at index {} is invalid", i);
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto metadata_zbi_type = metadata.zbi_type();
    ZX_ASSERT(metadata_zbi_type.has_value());

    zx::result data =
        bus_->GetBootItemArray(metadata_zbi_type.value(), metadata.zbi_extra().value());
    if (data.is_ok()) {
      // TODO(b/341981272): Remove `device_server_.AddMetadata()` once all drivers bound to
      // platform devices do not use `device_get_metadata()` to retrieve metadata.
      zx_status_t status =
          device_server_->AddMetadata(metadata_zbi_type.value(), data->data(), data->size());
      if (status != ZX_OK) {
        fdf::warn("Failed to add boot metadata with ZBI type {}: {}", metadata_zbi_type.value(),
                  zx_status_get_string(status));
      }

      metadata_.emplace(std::to_string(metadata_zbi_type.value()),
                        std::vector<uint8_t>{data->begin(), data->end()});

      switch (metadata_zbi_type.value()) {
        case ZBI_TYPE_SERIAL_NUMBER: {
          auto metadata = CreateSerialNumberMetadata(data.value());
          serial_number_metadata_server_ = std::make_unique<
              fdf_metadata::MetadataServer<fuchsia_boot_metadata::SerialNumberMetadata>>(name_);
          if (zx::result result = serial_number_metadata_server_->Serve(
                  *bus()->outgoing(), bus()->dispatcher(), metadata);
              result.is_error()) {
            fdf::error("Failed to set metadata for serial number metadata server: {}", result);
            return result.take_error();
          }
          break;
        }
        case ZBI_TYPE_DRV_PARTITION_MAP: {
          zx::result metadata = CreatePartitionMap(data.value());
          if (metadata.is_error()) {
            fdf::error("Failed to create partition map metadata: {}", metadata);
            return metadata.take_error();
          }
          partition_map_metadata_server_ =
              std::make_unique<fdf_metadata::MetadataServer<fuchsia_boot_metadata::PartitionMap>>(
                  name_);
          if (zx::result result = partition_map_metadata_server_->Serve(
                  *bus()->outgoing(), bus()->dispatcher(), metadata.value());
              result.is_error()) {
            fdf::error("Failed to set metadata for partition map metadata server: {}", result);
            return result.take_error();
          }
          break;
        }
        case ZBI_TYPE_DRV_MAC_ADDRESS: {
          zx::result metadata = CreateMacAddressMetadata(data.value());
          if (metadata.is_error()) {
            fdf::error("Failed to create mac address metadata: {}", metadata);
            return metadata.take_error();
          }
          mac_address_metadata_server_ = std::make_unique<
              fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>>(name_);
          if (zx::result result = mac_address_metadata_server_->Serve(
                  *bus()->outgoing(), bus()->dispatcher(), metadata.value());
              result.is_error()) {
            fdf::error("Failed to set metadata for mac address metadata server: {}", result);
            return result.take_error();
          }
          break;
        }
        default:
          fdf::info("Ignoring boot metadata with zbi type {}", metadata_zbi_type.value());
          break;
      }
    }
  }

  return zx::ok();
}

void PlatformDevice::GetMmioById(GetMmioByIdRequestView request,
                                 GetMmioByIdCompleter::Sync& completer) {
  zx::result mmio_result = GetMmio(request->index);
  if (mmio_result.is_error()) {
    completer.ReplyError(mmio_result.status_value());
    return;
  }

  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_hardware_platform_device::wire::Mmio::Builder(arena)
                             .offset(mmio_result->offset)
                             .size(mmio_result->size)
                             .vmo(zx::vmo(mmio_result->vmo))
                             .Build());
}

void PlatformDevice::GetMmioByName(GetMmioByNameRequestView request,
                                   GetMmioByNameCompleter::Sync& completer) {
  if (request->name.empty()) {
    return completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }
  std::optional<uint32_t> index = GetMmioIndex(node_, request->name.get());
  if (!index.has_value()) {
    return completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
  }

  zx::result mmio_result = GetMmio(index.value());
  if (mmio_result.is_error()) {
    completer.ReplyError(mmio_result.status_value());
    return;
  }

  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_hardware_platform_device::wire::Mmio::Builder(arena)
                             .offset(mmio_result->offset)
                             .size(mmio_result->size)
                             .vmo(zx::vmo(mmio_result->vmo))
                             .Build());
}

void PlatformDevice::GetInterruptById(GetInterruptByIdRequestView request,
                                      GetInterruptByIdCompleter::Sync& completer) {
  GetInterrupt(request->index, request->flags, MakeGetInterruptCallback(completer));
}

void PlatformDevice::GetInterruptByName(GetInterruptByNameRequestView request,
                                        GetInterruptByNameCompleter::Sync& completer) {
  if (request->name.empty()) {
    return completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }
  std::optional<uint32_t> index = GetIrqIndex(node_, request->name.get());
  if (!index.has_value()) {
    return completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
  }

  GetInterrupt(index.value(), request->flags, MakeGetInterruptCallback(completer));
}

void PlatformDevice::GetBtiById(GetBtiByIdRequestView request,
                                GetBtiByIdCompleter::Sync& completer) {
  zx::result bti = GetBti(request->index);
  if (bti.is_ok()) {
    completer.ReplySuccess(std::move(bti.value()));
  } else {
    completer.ReplyError(bti.status_value());
  }
}

void PlatformDevice::GetBtiByName(GetBtiByNameRequestView request,
                                  GetBtiByNameCompleter::Sync& completer) {
  if (request->name.empty()) {
    return completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }
  std::optional<uint32_t> index = GetBtiIndex(node_, request->name.get());
  if (!index.has_value()) {
    return completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
  }
  zx::result bti = GetBti(index.value());
  if (bti.is_ok()) {
    completer.ReplySuccess(std::move(bti.value()));
  } else {
    completer.ReplyError(bti.status_value());
  }
}

void PlatformDevice::GetSmcById(GetSmcByIdRequestView request,
                                GetSmcByIdCompleter::Sync& completer) {
  zx::result smc = GetSmc(request->index);
  if (smc.is_ok()) {
    completer.ReplySuccess(std::move(smc.value()));
  } else {
    completer.ReplyError(smc.status_value());
  }
}

void PlatformDevice::GetSmcByName(GetSmcByNameRequestView request,
                                  GetSmcByNameCompleter::Sync& completer) {
  if (request->name.empty()) {
    return completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }
  std::optional<uint32_t> index = GetSmcIndex(node_, request->name.get());
  if (!index.has_value()) {
    return completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
  }
  zx::result smc = GetSmc(index.value());
  if (smc.is_ok()) {
    completer.ReplySuccess(std::move(smc.value()));
  } else {
    completer.ReplyError(smc.status_value());
  }
}

void PlatformDevice::GetPowerConfiguration(GetPowerConfigurationCompleter::Sync& completer) {
  std::optional<std::vector<fuchsia_hardware_power::PowerElementConfiguration>> config =
      node_.power_config();
  if (config.has_value()) {
    auto element_configs = config.value();
    fidl::Arena arena;
    fidl::VectorView<fuchsia_hardware_power::wire::PowerElementConfiguration> elements;
    elements.Allocate(arena, element_configs.size());

    size_t offset = 0;
    for (auto& config : element_configs) {
      fuchsia_hardware_power::wire::PowerElementConfiguration wire_config =
          fidl::ToWire(arena, config);
      elements.at(offset) = wire_config;

      offset++;
    }
    completer.ReplySuccess(elements);

  } else {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  }
}

void PlatformDevice::GetNodeDeviceInfo(GetNodeDeviceInfoCompleter::Sync& completer) {
  DeviceInfo device_info = GetDeviceInfo();
  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_hardware_platform_device::wire::NodeDeviceInfo::Builder(arena)
                             .vid(device_info.vid)
                             .pid(device_info.pid)
                             .did(device_info.did)
                             .mmio_count(device_info.mmio_count)
                             .irq_count(device_info.irq_count)
                             .bti_count(device_info.bti_count)
                             .smc_count(device_info.smc_count)
                             .metadata_count(device_info.metadata_count)
                             .name(device_info.name)
                             .Build());
}

void PlatformDevice::GetBoardInfo(GetBoardInfoCompleter::Sync& completer) {
  BoardInfo board_info = GetBoardInfo();
  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_hardware_platform_device::wire::BoardInfo::Builder(arena)
                             .vid(board_info.vid)
                             .pid(board_info.pid)
                             .board_name(board_info.board_name)
                             .board_revision(board_info.board_revision)
                             .Build());
}

void PlatformDevice::GetMetadata(GetMetadataRequestView request,
                                 GetMetadataCompleter::Sync& completer) {
  if (auto metadata = metadata_.find(request->id.get()); metadata != metadata_.end()) {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(metadata->second));
    return;
  }

  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void PlatformDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_platform_device::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("PlatformDevice received unknown method with ordinal: {}", metadata.method_ordinal);
}

void PlatformDevice::RegisterController(RegisterControllerRequestView request,
                                        RegisterControllerCompleter::Sync& completer) {
  if (!node_.interrupt_controller_id().has_value()) {
    fdf::error(
        "RegisterController called on device \"{}\" which is not configured as an interrupt controller",
        name_);
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  const uint32_t controller_id = node_.interrupt_controller_id().value();
  zx::result<> result =
      bus_->RegisterInterruptController(controller_id, std::move(request->controller));
  completer.Reply(result);
}

void PlatformDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_interrupt::ControllerRegistry> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("PlatformDevice received unknown method with ordinal: {}", metadata.method_ordinal);
}

}  // namespace platform_bus
