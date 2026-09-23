// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/board/lib/acpi/device-builder.h"

#include <lib/ddk/debug.h>
#include <zircon/compiler.h>

#include <bind/fuchsia/acpi/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pci/cpp/bind.h>
#include <fbl/string_printf.h>

#ifdef __Fuchsia__
#include "src/devices/board/lib/acpi/device.h"
#include "src/devices/board/lib/acpi/irq-fragment.h"
#else
#include "src/devices/board/lib/acpi/device-for-host.h"
#endif
#include "src/devices/board/lib/acpi/manager.h"
#include "src/devices/board/lib/acpi/resources.h"
#include "src/devices/lib/acpi/util.h"

namespace acpi {
namespace {

device_bind_prop_t MakeProperty(zx_device_str_prop_t str_prop) {
  switch (str_prop.property_value.data_type) {
    case ZX_DEVICE_PROPERTY_VALUE_STRING:
      return ddk::MakeProperty(str_prop.key, str_prop.property_value.data.str_val);
    case ZX_DEVICE_PROPERTY_VALUE_BOOL:
      return ddk::MakeProperty(str_prop.key, str_prop.property_value.data.bool_val);
    case ZX_DEVICE_PROPERTY_VALUE_INT:
      return ddk::MakeProperty(str_prop.key, str_prop.property_value.data.int_val);
    case ZX_DEVICE_PROPERTY_VALUE_ENUM:
      return ddk::MakeProperty(str_prop.key, str_prop.property_value.data.enum_val);
    default:
      ZX_PANIC("Unknown property type: %d", str_prop.property_value.data_type);
  }
}

ddk::BindRule MakeAcceptBindRule(zx_device_str_prop_t str_prop) {
  device_bind_prop_value_t value;
  switch (str_prop.property_value.data_type) {
    case ZX_DEVICE_PROPERTY_VALUE_STRING:
      value = device_bind_prop_str_val(str_prop.property_value.data.str_val);
      break;
    case ZX_DEVICE_PROPERTY_VALUE_BOOL:
      value = device_bind_prop_bool_val(str_prop.property_value.data.bool_val);
      break;
    case ZX_DEVICE_PROPERTY_VALUE_INT:
      value = device_bind_prop_int_val(str_prop.property_value.data.int_val);
      break;
    case ZX_DEVICE_PROPERTY_VALUE_ENUM:
      value = device_bind_prop_enum_val(str_prop.property_value.data.enum_val);
      break;
    default:
      ZX_PANIC("Unknown property type: %d", str_prop.property_value.data_type);
  }
  return ddk::BindRule(device_bind_prop_str_key(str_prop.key), DEVICE_BIND_RULE_CONDITION_ACCEPT,
                       value);
}

}  // namespace

acpi::status<> DeviceBuilder::GatherResources(acpi::Acpi* acpi, acpi::Manager* manager,
                                              GatherResourcesCallback callback) {
  if (!handle_ || !parent_) {
    // Skip the root device.
    return acpi::ok();
  }

  // Don't decode resources if the ENABLED bit is not set.
  // See ACPI v6.4 section 6.3.7
  if (!(state_ & ACPI_STA_DEVICE_ENABLED)) {
    return acpi::ok();
  }

  // TODO(https://fxbug.dev/42158705): Handle other resources like serial buses.
  auto result = acpi->WalkResources(
      handle_, "_CRS", [this, acpi, manager, &callback](ACPI_RESOURCE* res) -> acpi::status<> {
        ACPI_HANDLE bus_parent = nullptr;
        BusType type = BusType::kUnknown;
        DeviceChildEntry entry;
        const char* bus_id_prop;
        if (resource_is_spi(res)) {
          type = BusType::kSpi;
          auto result = resource_parse_spi(acpi, handle_, res, &bus_parent);
          if (result.is_error()) {
            zxlogf(WARNING, "Failed to parse SPI resource: %d", result.error_value());
            return result.take_error();
          }
          entry = result.value();
          bus_id_prop = bind_fuchsia::ID;
          str_props_.emplace_back(OwnedStringProp(bind_fuchsia::NAME, "spi"));
        } else if (resource_is_i2c(res)) {
          type = BusType::kI2c;
          auto result = resource_parse_i2c(acpi, handle_, res, &bus_parent);
          if (result.is_error()) {
            zxlogf(WARNING, "Failed to parse I2C resource: %d", result.error_value());
            return result.take_error();
          }
          entry = result.value();
          bus_id_prop = bind_fuchsia::ID;
          str_props_.emplace_back(OwnedStringProp(bind_fuchsia::NAME, "i2c"));
        } else if (resource_is_irq(res)) {
          irq_count_++;
        }

        if (bus_parent) {
          size_t bus_index = callback(bus_parent, type, entry);
          DeviceBuilder* b = manager->LookupDevice(bus_parent);
          buses_.emplace_back(b, bus_index);
          str_props_.emplace_back(OwnedStringProp(bus_id_prop, b->GetBusId()));
          has_address_ = true;
        }
        return acpi::ok();
      });
  if (result.is_error() && result.zx_status_value() != ZX_ERR_NOT_FOUND) {
    return result.take_error();
  }

  auto info = acpi->GetObjectInfo(handle_);
  if (info.is_error()) {
    zxlogf(WARNING, "Failed to get object info: %d", info.status_value());
    return info.take_error();
  }

  // PCI is special, and PCI devices don't have an explicit resource. Instead, we need to check
  // _ADR for PCI addressing info.
  if (parent_->bus_type_ == BusType::kPci) {
    if (info->Valid & ACPI_VALID_ADR) {
      callback(parent_->handle_, BusType::kPci, DeviceChildEntry(info->Address));
      // Set up some bind properties for ourselves. callback() should set HasBusId.
      ZX_ASSERT(parent_->HasBusId());
      uint32_t bus_id = parent_->GetBusId();
      uint32_t device = (info->Address & (0xffff0000)) >> 16;
      uint32_t func = info->Address & 0x0000ffff;
      str_props_.emplace_back(
          OwnedStringProp(bind_fuchsia::PCI_TOPO, BIND_PCI_TOPO_PACK(bus_id, device, func)));
      // Should we buses_.emplace_back() here? The PCI bus driver currently publishes PCI
      // composites, so having a device on a PCI bus that uses other buses resources can't be
      // represented. Such devices don't seem to exist, but if we ever encounter one, it will need
      // to be handled somehow.
      has_address_ = true;
    }
  }

  bool has_devicetree_cid = false;
  // Add HID and CID properties, if present.
  if (info->Valid & ACPI_VALID_HID) {
    if (!strcmp(info->HardwareId.String, kDeviceTreeLinkID)) {
      has_devicetree_cid = CheckForDeviceTreeCompatible(acpi);
    } else {
      str_props_.emplace_back(OwnedStringProp(bind_fuchsia_acpi::HID, info->HardwareId.String));
    }
  }

  if (!has_devicetree_cid && info->Valid & ACPI_VALID_CID && info->CompatibleIdList.Count > 0) {
    auto& first = info->CompatibleIdList.Ids[0];
    if (!strcmp(first.String, kDeviceTreeLinkID)) {
      has_devicetree_cid = CheckForDeviceTreeCompatible(acpi);
    } else {
      // We only expose the first CID.
      str_props_.emplace_back(OwnedStringProp(bind_fuchsia_acpi::FIRST_CID, first.String));
    }
  }

  // If our parent has a bus type, and we have an address on that bus, then we'll expose it in our
  // bind properties.
  if (parent_->GetBusType() != BusType::kUnknown && has_address_) {
    str_props_.emplace_back(OwnedStringProp(bind_fuchsia::ACPI_BUS_TYPE, parent_->GetBusType()));
  }
  if (result.status_value() == AE_NOT_FOUND) {
    return acpi::ok();
  }
  return result;
}

zx::result<zx_device_t*> DeviceBuilder::Build(acpi::Manager* manager,
                                              async_dispatcher_t* device_dispatcher) {
  if (parent_->zx_device_ == nullptr) {
    zxlogf(ERROR, "Parent has not been added to the tree yet!");
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (zx_device_ != nullptr) {
    zxlogf(ERROR, "This device (%s) has already been built!", name());
    return zx::error(ZX_ERR_BAD_STATE);
  }
  DeviceArgs device_args(parent_->zx_device_, manager, device_dispatcher, handle_);
  if (HasBusId() && bus_type_ != BusType::kPci) {
    zx::result metadata = GetMetadata();
    if (metadata.is_error()) {
      zxlogf(ERROR, "Failed to get metadata for '%s': %d", name(), metadata.status_value());
      return metadata.take_error();
    }
    device_args.SetBusMetadata(std::move(*metadata), bus_type_, GetBusId());
  }
  auto device = std::make_unique<Device>(std::move(device_args));

  // Narrow our custom type down to zx_device_str_prop_t.
  // Any strings in zx_device_str_prop_t will still point at their equivalents
  // in the original str_props_ array.
  std::vector<zx_device_str_prop_t> str_props_for_ddkadd;
  for (auto& str_prop : str_props_) {
    str_props_for_ddkadd.emplace_back(str_prop);
  }

  uint32_t add_flags = DEVICE_ADD_MUST_ISOLATE;
  if ((state_ & (ACPI_STA_DEVICE_FUNCTIONING | ACPI_STA_DEVICE_PRESENT)) ==
      ACPI_STA_DEVICE_FUNCTIONING) {
    // Don't bind drivers to this device if it is functioning but not present.
    // See ACPI 6.4 section 6.3.7.
    add_flags |= DEVICE_ADD_NON_BINDABLE;
  }

  auto result = device->AddDevice(name(), cpp20::span(str_props_for_ddkadd), add_flags);
  if (result.is_error()) {
    zxlogf(ERROR, "failed to publish acpi device '%s' (parent=%s): %d", name(), parent_->name(),
           result.status_value());
    return result.take_error();
  }

  auto* acpi_dev = device.release();
  zx_device_ = acpi_dev->zxdev();

  for (uint32_t i = 0; i < irq_count_; i++) {
#ifdef __Fuchsia__
    auto result = IrqFragment::Create(device_dispatcher, *acpi_dev, i, device_id_);
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to construct IRQ fragment: %d", result.status_value());
      return result.take_error();
    }
#endif
  }

  auto status = BuildComposite(manager, str_props_for_ddkadd, device_dispatcher);
  if (status.is_error()) {
    zxlogf(WARNING, "failed to publish composite acpi device '%s-composite': %d", name(),
           status.error_value());
    return status.take_error();
  }

  return zx::ok(zx_device_);
}

size_t DeviceBuilder::AddBusChild(DeviceChildEntry d) {
  return std::visit(
      [this](auto&& arg) {
        using T = std::decay_t<decltype(arg)>;
        // If we haven't initialised the vector yet, populate it.
        auto pval_empty = std::get_if<std::monostate>(&bus_children_);
        if (pval_empty) {
          auto tmp = DeviceChildData(std::vector<T>());
          bus_children_.swap(tmp);
        }

        auto pval = std::get_if<std::vector<T>>(&bus_children_);
        ZX_ASSERT_MSG(pval, "Bus %s had unexpected child type vector", name());
        pval->emplace_back(arg);
        return pval->size() - 1;
      },
      d);
}

zx::result<BusMetadata> DeviceBuilder::GetMetadata() {
#ifdef __Fuchsia__
  using SpiChannel = fuchsia_hardware_spi_businfo::SpiChannel;
  using I2CChannel = fuchsia_hardware_i2c_businfo::I2CChannel;
  return std::visit(
      [this](DeviceChildData&& arg) -> zx::result<BusMetadata> {
        if (std::holds_alternative<std::monostate>(arg)) {
          return zx::ok(std::monostate{});
        }

        if (std::holds_alternative<std::vector<SpiChannel>>(arg)) {
          ZX_ASSERT(HasBusId());  // Bus ID should get set when a child device is added.
          return zx::ok(fuchsia_hardware_spi_businfo::SpiBusMetadata{{
              .channels = std::get<std::vector<SpiChannel>>(arg),
              .bus_id = GetBusId(),
          }});
        }

        if (std::holds_alternative<std::vector<I2CChannel>>(arg)) {
          ZX_ASSERT(HasBusId());  // Bus ID should get set when a child device is added.
          return zx::ok(fuchsia_hardware_i2c_businfo::I2CBusMetadata{{
              .channels = std::get<std::vector<I2CChannel>>(arg),
              .bus_id = GetBusId(),
          }});
        }

        return zx::error(ZX_ERR_NOT_SUPPORTED);
      },
      bus_children_);
#else  // __Fuchsia__
  return zx::error(ZX_ERR_NOT_SUPPORTED);
#endif
}

zx::result<> DeviceBuilder::BuildComposite(acpi::Manager* manager,
                                           std::vector<zx_device_str_prop_t>& str_props,
                                           async_dispatcher_t* device_dispatcher) {
  if (parent_->GetBusType() == BusType::kPci) {
    // If a device is on a PCI bus, the PCI bus driver will publish a composite device, so we
    // don't try to publish a composite.
    return zx::ok();
  }

  auto [acpi_bind_rules, acpi_properties] = GetFragmentBindRulesAndPropertiesForSelf();
  for (const auto& str_prop : str_props) {
    acpi_properties.emplace_back(MakeProperty(str_prop));
  }
  auto composite_node_spec = ddk::CompositeNodeSpec(acpi_bind_rules, acpi_properties);

  // Generate composite node spec parent for every device we use.
  for (auto& pair : buses_) {
    DeviceBuilder* parent = pair.first;
    size_t child_index = pair.second;

    auto [bind_rules, properties] = parent->GetFragmentBindRulesAndPropertiesForChild(child_index);
    composite_node_spec.AddParentSpec(bind_rules, properties);
  }

  for (uint32_t i = 0; i < irq_count_; i++) {
    auto bind_platform_dev_interrupt_id = i + 1;
    composite_node_spec.AddParentSpec(
        std::vector<ddk::BindRule>{
            ddk::MakeAcceptBindRule(bind_fuchsia::ACPI_ID, device_id_),
            ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INTERRUPT_ID,
                                    bind_platform_dev_interrupt_id),
            ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.interrupt.Service")},
        std::vector<device_bind_prop_t>{
            ddk::MakeProperty(bind_fuchsia::ACPI_ID, device_id_),
            ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_INTERRUPT_ID,
                              bind_platform_dev_interrupt_id),
            ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.interrupt.Service")});
  }

#if !defined(IS_TEST)
  // TODO(https://fxbug.dev/42160209): re-enable this in tests once mock_ddk supports composites.
  fbl::String spec_name;
  uint32_t id = manager->GetNextCompositeId(name());
  if (id == 0) {
    spec_name = fbl::StringPrintf("%s-composite-spec", name());
  } else {
    spec_name = fbl::StringPrintf("%s-%i-composite-spec", name(), id);
  }

  DeviceArgs composite_node_spec_args(parent_->zx_device_, manager, device_dispatcher, handle_);
  auto composite_node_spec_device = std::make_unique<Device>(composite_node_spec_args);
  zx_status_t status =
      composite_node_spec_device->DdkAddCompositeNodeSpec(spec_name.data(), composite_node_spec);
  if (status != ZX_OK) {
#ifdef __Fuchsia__
    zxlogf(ERROR, "Failed to add composite node spec: %s", zx_status_get_string(status));
#else
    zxlogf(ERROR, "Failed to add composite node spec");
#endif
    return zx::error(status);
  }

  // The DDK takes ownership of the device.
  [[maybe_unused]] auto* unused = composite_node_spec_device.release();
#endif

  return zx::ok();
}

std::pair<std::vector<ddk::BindRule>, std::vector<device_bind_prop_t>>
DeviceBuilder::GetFragmentBindRulesAndPropertiesForChild(size_t child_index) {
  std::vector<ddk::BindRule> bind_rules;
  std::vector<device_bind_prop_t> properties;

  switch (bus_type_) {
    case BusType::kPci:
      bind_rules.emplace_back(
          ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_pci::BIND_PROTOCOL_DEVICE));
      properties.emplace_back(
          ddk::MakeProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_pci::BIND_PROTOCOL_DEVICE));
      break;
    case BusType::kI2c:
      // No Banjo protocol needed for I2C.
      break;
    case BusType::kSpi:
      bind_rules.emplace_back(
          ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"));
      properties.emplace_back(
          ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"));
      break;
    case BusType::kUnknown:
      ZX_PANIC("Bus type is unknown");
  }

  std::visit(
      [&bind_rules, &properties, child_index](auto&& arg) {
        using T = std::decay_t<decltype(arg)>;
        using SpiChannel = fuchsia_hardware_spi_businfo::SpiChannel;
        using I2CChannel = fuchsia_hardware_i2c_businfo::I2CChannel;
        if constexpr (std::is_same_v<T, std::monostate>) {
          ZX_PANIC("Bus should have children");
        } else if constexpr (std::is_same_v<T, std::vector<SpiChannel>>) {
          SpiChannel& chan = arg[child_index];
          if (chan.global_id().has_value()) {
            bind_rules.emplace_back(ddk::MakeAcceptBindRule(bind_fuchsia::ID, *chan.global_id()));
            properties.emplace_back(ddk::MakeProperty(bind_fuchsia::ID, *chan.global_id()));
          }
          properties.emplace_back(ddk::MakeProperty(bind_fuchsia::NAME, "spi"));
        } else if constexpr (std::is_same_v<T, std::vector<I2CChannel>>) {
          I2CChannel& chan = arg[child_index];
          if (chan.global_id().has_value()) {
            bind_rules.emplace_back(ddk::MakeAcceptBindRule(bind_fuchsia::ID, *chan.global_id()));
            properties.emplace_back(ddk::MakeProperty(bind_fuchsia::ID, *chan.global_id()));
          }
          bind_rules.emplace_back(
              ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"));
          properties.emplace_back(
              ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"));
          properties.emplace_back(ddk::MakeProperty(bind_fuchsia::NAME, "i2c"));
        }
      },
      bus_children_);

  return {bind_rules, properties};
}

std::pair<std::vector<ddk::BindRule>, std::vector<device_bind_prop_t>>
DeviceBuilder::GetFragmentBindRulesAndPropertiesForSelf() {
  std::vector<ddk::BindRule> bind_rules = {
      ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_acpi::BIND_PROTOCOL_DEVICE),
  };
  std::vector<device_bind_prop_t> properties = {
      ddk::MakeProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_acpi::BIND_PROTOCOL_DEVICE),
  };

  for (auto& prop : str_props_) {
    bind_rules.emplace_back(MakeAcceptBindRule(prop));
    properties.emplace_back(MakeProperty(prop));
  }

  return {bind_rules, properties};
}

bool DeviceBuilder::CheckForDeviceTreeCompatible(acpi::Acpi* acpi) {
  // UUID defined in "Device Properties UUID for _DSD", Revision 2.0, Section 2.1
  // https://uefi.org/sites/default/files/resources/_DSD-device-properties-UUID.pdf
  static constexpr Uuid kDevicePropertiesUuid =
      Uuid::Create(0xdaffd814, 0x6eba, 0x4d8c, 0x8a91, 0xbc9bbf4aa301);
  auto result = acpi->EvaluateObject(handle_, "_DSD", std::nullopt);
  if (result.is_error()) {
    if (result.zx_status_value() != ZX_ERR_NOT_FOUND) {
      zxlogf(WARNING, "Get _DSD for '%s' failed: %d", name(), result.error_value());
    }
    return false;
  }

  auto value = std::move(result.value());
  if (value->Type != ACPI_TYPE_PACKAGE) {
    zxlogf(WARNING, "'%s': Badly formed _DSD return value - wrong data type", name());
    return false;
  }

  // The package is an array of pairs. The first item in each pair is a UUID, and the second is the
  // value of that UUID.
  ACPI_OBJECT* properties = nullptr;
  for (size_t i = 0; (i + 1) < value->Package.Count; i += 2) {
    ACPI_OBJECT* uuid_buffer = &value->Package.Elements[i];
    if (uuid_buffer->Type != ACPI_TYPE_BUFFER || uuid_buffer->Buffer.Length != acpi::kUuidBytes) {
      zxlogf(WARNING, "'%s': _DSD entry %zu has invalid UUID.", name(), i);
      continue;
    }

    if (!memcmp(uuid_buffer->Buffer.Pointer, kDevicePropertiesUuid.bytes, acpi::kUuidBytes)) {
      properties = &value->Package.Elements[i + 1];
      break;
    }
  }

  if (!properties) {
    return false;
  }

  if (properties->Type != ACPI_TYPE_PACKAGE) {
    zxlogf(WARNING, "'%s': Device Properties _DSD value is not a package.", name());
    return false;
  }

  // properties should be a list of packages, which are each a key/value pair.
  for (size_t i = 0; i < properties->Package.Count; i++) {
    ACPI_OBJECT* pair = &properties->Package.Elements[i];
    if (pair->Type != ACPI_TYPE_PACKAGE || pair->Package.Count != 2) {
      continue;
    }

    ACPI_OBJECT* key = &pair->Package.Elements[0];
    ACPI_OBJECT* value = &pair->Package.Elements[1];
    if (key->Type != ACPI_TYPE_STRING || key->String.Length < sizeof("compatible") - 1) {
      continue;
    }

    if (!strcmp("compatible", key->String.Pointer) && value->Type == ACPI_TYPE_STRING) {
      str_props_.emplace_back(OwnedStringProp{"fuchsia.acpi.FIRST_CID", value->String.Pointer});
      return true;
    }
  }
  return false;
}
}  // namespace acpi
