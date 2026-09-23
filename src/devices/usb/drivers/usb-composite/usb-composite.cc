// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-composite/usb-composite.h"

#include <endian.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <stdio.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/usb/cpp/bind.h>
#include <fbl/auto_lock.h>

#include "src/devices/usb/drivers/usb-composite/usb-interface.h"

namespace usb_composite {

zx_status_t UsbComposite::Create(void* ctx, zx_device_t* parent) {
  fbl::AllocChecker ac;
  auto device = fbl::make_unique_checked<UsbComposite>(&ac, parent);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  auto status = device->Init();
  if (status != ZX_OK) {
    return status;
  }

  // Device manager is now in charge of the device.
  [[maybe_unused]] auto* unowned_device = device.release();

  return ZX_OK;
}

void UsbComposite::RemoveInterface(UsbInterface* interface) {
  size_t index = 0;
  fbl::AutoLock _(&lock_);
  for (auto intf : interfaces_) {
    if (intf == interface) {
      interfaces_.erase(index);
      return;
    }
    index++;
  }
}

// returns whether the interface with the given id was removed.
bool UsbComposite::RemoveInterfaceById(uint8_t interface_id) {
  size_t index = 0;
  for (auto intf : interfaces_) {
    if (intf->ContainsInterface(interface_id)) {
      intf->DdkAsyncRemove();
      interfaces_.erase(index);
      return true;
    }
    index++;
  }
  return false;
}

zx_status_t UsbComposite::AddInterface(const usb_interface_descriptor_t* interface_desc,
                                       size_t length) {
  auto client = DdkConnectFidlProtocol<fuchsia_hardware_usb::UsbService::Device>();
  if (client.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol");
    return client.error_value();
  }

  std::unique_ptr<UsbInterface> interface;
  auto client_end =
      UsbInterface::Create(zxdev(), this, usb_, std::move(*client), interface_desc, length,
                           fdf::Dispatcher::GetCurrent()->async_dispatcher(), &interface);
  if (client_end.is_error()) {
    return client_end.error_value();
  }

  {
    fbl::AutoLock lock(&lock_);
    // We need to do this first before calling DdkAdd().
    interfaces_.push_back(interface.get());
  }

  char name[20];
  snprintf(name, sizeof(name), "ifc-%03d", interface_desc->b_interface_number);

  zx_device_str_prop_t props[] = {
      ddk::MakeStrProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_usb::BIND_PROTOCOL_INTERFACE),
      ddk::MakeStrProperty(bind_fuchsia::USB_VID, static_cast<uint32_t>(device_desc_.id_vendor)),
      ddk::MakeStrProperty(bind_fuchsia::USB_PID, static_cast<uint32_t>(device_desc_.id_product)),
      ddk::MakeStrProperty(bind_fuchsia::USB_CLASS, static_cast<uint32_t>(interface->usb_class())),
      ddk::MakeStrProperty(bind_fuchsia::USB_SUBCLASS,
                           static_cast<uint32_t>(interface->usb_subclass())),
      ddk::MakeStrProperty(bind_fuchsia::USB_PROTOCOL,
                           static_cast<uint32_t>(interface->usb_protocol())),
      ddk::MakeStrProperty(bind_fuchsia::USB_INTERFACE_NUMBER,
                           static_cast<uint32_t>(interface_desc->b_interface_number)),
  };

  std::array offers = {
      fuchsia_hardware_usb::UsbService::Name,
  };

  auto status = interface->DdkAdd(ddk::DeviceAddArgs(name)
                                      .set_str_props(props)
                                      .set_fidl_service_offers(offers)
                                      .set_outgoing_dir(client_end->TakeChannel()));
  if (status == ZX_OK) {
    // Hold a reference while devmgr has a pointer to this object.
    interface.release();
  } else {
    fbl::AutoLock lock(&lock_);
    interfaces_.pop_back();
  }

  return status;
}

zx_status_t UsbComposite::AddInterfaceAssoc(const usb_interface_assoc_descriptor_t* assoc_desc,
                                            size_t length) {
  auto client = DdkConnectFidlProtocol<fuchsia_hardware_usb::UsbService::Device>();
  if (client.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol");
    return client.error_value();
  }

  std::unique_ptr<UsbInterface> interface;
  auto client_end =
      UsbInterface::Create(zxdev(), this, usb_, std::move(*client), assoc_desc, length,
                           fdf::Dispatcher::GetCurrent()->async_dispatcher(), &interface);
  if (client_end.is_error()) {
    return client_end.error_value();
  }

  {
    fbl::AutoLock lock(&lock_);
    // We need to do this first before calling DdkAdd().
    interfaces_.push_back(interface.get());
  }

  char name[20];
  snprintf(name, sizeof(name), "asc-%03d", assoc_desc->b_first_interface);

  zx_device_str_prop_t props[] = {
      ddk::MakeStrProperty(bind_fuchsia::PROTOCOL,
                           bind_fuchsia_usb::BIND_PROTOCOL_INTERFACE_ASSOCIATION),
      ddk::MakeStrProperty(bind_fuchsia::USB_VID, static_cast<uint32_t>(device_desc_.id_vendor)),
      ddk::MakeStrProperty(bind_fuchsia::USB_PID, static_cast<uint32_t>(device_desc_.id_product)),
      ddk::MakeStrProperty(bind_fuchsia::USB_CLASS, static_cast<uint32_t>(interface->usb_class())),
      ddk::MakeStrProperty(bind_fuchsia::USB_SUBCLASS,
                           static_cast<uint32_t>(interface->usb_subclass())),
      ddk::MakeStrProperty(bind_fuchsia::USB_PROTOCOL,
                           static_cast<uint32_t>(interface->usb_protocol())),
      ddk::MakeStrProperty(bind_fuchsia::USB_INTERFACE_NUMBER,
                           static_cast<uint32_t>(assoc_desc->b_first_interface)),
  };

  std::array offers = {
      fuchsia_hardware_usb::UsbService::Name,
  };

  auto status = interface->DdkAdd(ddk::DeviceAddArgs(name)
                                      .set_str_props(props)
                                      .set_fidl_service_offers(offers)
                                      .set_outgoing_dir(client_end->TakeChannel()));
  if (status == ZX_OK) {
    // Hold a reference while devmgr has a pointer to this object.
    interface.release();
  } else {
    fbl::AutoLock lock(&lock_);
    interfaces_.pop_back();
  }

  return status;
}

zx_status_t UsbComposite::AddInterfaces() {
  if (config_desc_.size() < sizeof(usb_configuration_descriptor_t)) {
    zxlogf(ERROR, "Malformed USB descriptor detected!");
    return ZX_ERR_INTERNAL;
  }
  auto* config = reinterpret_cast<usb_configuration_descriptor_t*>(config_desc_.data());
  const usb::UnownedInterfaceList interface_list(config, le16toh(config->w_total_length), true);
  zx_status_t assoc_status = ZX_OK;
  struct assoc_iterator_t {
    const usb_interface_assoc_descriptor_t* desc_;
    size_t length_;
    UsbComposite* dev_ptr_;
    zx_status_t* status_;

    assoc_iterator_t(const usb_interface_assoc_descriptor_t* desc, size_t length,
                     UsbComposite* dev_ptr, zx_status_t* status)
        : desc_(desc), length_(length), dev_ptr_(dev_ptr), status_(status) {}

    ~assoc_iterator_t() {
      auto status = dev_ptr_->AddInterfaceAssoc(desc_, length_);
      if (status_ && *status_ != ZX_OK) {
        *status_ = status;
      }
    }
  };
  std::optional<assoc_iterator_t> assoc = std::nullopt;
  for (const auto& interface : interface_list) {
    if (!interface.descriptor()) {
      zxlogf(ERROR, "Malformed USB descriptor detected!");
      return ZX_ERR_INTERNAL;
    }

    if (interface.association()) {
      if (!assoc || interface.association() != assoc->desc_) {
        assoc.emplace(interface.association(), sizeof(*interface.association()), this,
                      &assoc_status);
      }

      assoc->length_ += interface.length(true);
      continue;
    }

    assoc.reset();
    if (assoc_status != ZX_OK) {
      return assoc_status;
    }

    auto intf_num = interface.descriptor()->b_interface_number;
    InterfaceStatus intf_status;
    {
      fbl::AutoLock lock(&lock_);

      // Only create a child device if no child interface has claimed this interface.
      intf_status = interface_statuses_[intf_num];
    }

    if (intf_status == InterfaceStatus::AVAILABLE) {
      auto status = AddInterface(interface.descriptor(), interface.length(true));
      if (status != ZX_OK) {
        return status;
      }

      fbl::AutoLock lock(&lock_);

      // The interface may have been claimed in the meanwhile, so we need to
      // check the interface status again.
      if (interface_statuses_[intf_num] == InterfaceStatus::CLAIMED) {
        bool removed = RemoveInterfaceById(intf_num);
        if (!removed) {
          return ZX_ERR_BAD_STATE;
        }
      } else {
        interface_statuses_[intf_num] = InterfaceStatus::CHILD_DEVICE;
      }
    }
  }

  return assoc_status;
}

UsbInterface* UsbComposite::GetInterfaceById(uint8_t interface_id) {
  for (auto intf : interfaces_) {
    if (intf->ContainsInterface(interface_id)) {
      return intf;
    }
  }
  return nullptr;
}

zx_status_t UsbComposite::ClaimInterface(uint8_t interface_id) {
  fbl::AutoLock lock(&lock_);

  auto intf = GetInterfaceById(interface_id);
  if (intf == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (interface_statuses_[interface_id] == InterfaceStatus::CLAIMED) {
    // The interface has already been claimed by a different interface.
    return ZX_ERR_ALREADY_BOUND;
  } else if (interface_statuses_[interface_id] == InterfaceStatus::CHILD_DEVICE) {
    bool removed = RemoveInterfaceById(interface_id);
    if (!removed) {
      return ZX_ERR_BAD_STATE;
    }
  }
  interface_statuses_[interface_id] = InterfaceStatus::CLAIMED;

  return ZX_OK;
}

zx_status_t UsbComposite::SetInterface(uint8_t interface_id, uint8_t alt_setting) {
  fbl::AutoLock lock(&lock_);

  for (auto intf : interfaces_) {
    if (intf->ContainsInterface(interface_id)) {
      return intf->SetAltSetting(interface_id, alt_setting);
    }
  }
  return ZX_ERR_INVALID_ARGS;
}

zx_status_t UsbComposite::GetAdditionalDescriptorList(uint8_t last_interface_id,
                                                      uint8_t* out_desc_list, size_t desc_count,
                                                      size_t* out_desc_actual) {
  *out_desc_actual = 0;

  if (config_desc_.size() < sizeof(usb_configuration_descriptor_t)) {
    zxlogf(ERROR, "Malformed USB descriptor detected!");
    return ZX_ERR_INTERNAL;
  }
  auto* config = reinterpret_cast<usb_configuration_descriptor_t*>(config_desc_.data());
  usb::UnownedInterfaceList intf_list(config, le16toh(config->w_total_length), true);
  for (const auto& intf : intf_list) {
    if (!intf.descriptor()) {
      zxlogf(ERROR, "Malformed USB descriptor detected!");
      return ZX_ERR_INTERNAL;
    }
    // We are only interested in descriptors past the last stored descriptor
    // for the current interface.
    if (intf.descriptor()->b_interface_number > last_interface_id) {
      size_t length = config->w_total_length - (reinterpret_cast<uintptr_t>(intf.descriptor()) -
                                                reinterpret_cast<uintptr_t>(config));
      if (length > desc_count) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      memcpy(out_desc_list, intf.descriptor(), length);
      *out_desc_actual = length;
      return ZX_OK;
    }
  }
  return ZX_OK;
}

void UsbComposite::DdkInit(ddk::InitTxn txn) {
  zx_status_t status = AddInterfaces();
  if (status != ZX_OK) {
    zxlogf(WARNING, "AddInterfaces() = %s", zx_status_get_string(status));
  }
  txn.Reply(status);
}

void UsbComposite::DdkUnbind(ddk::UnbindTxn txn) {
  {
    fbl::AutoLock lock(&lock_);
    interfaces_.reset();
  }

  txn.Reply();
}

void UsbComposite::DdkChildPreRelease(void* child_ctx) {
  {
    fbl::AutoLock lock(&lock_);
    for (size_t i = 0; i < interfaces_.size(); i++) {
      if (interfaces_[i] == child_ctx) {
        interfaces_.erase(i);
        break;
      }
    }
  }
}

void UsbComposite::DdkRelease() { delete this; }

zx_status_t UsbComposite::Init() {
  // Parent must support USB protocol.
  if (!usb_.is_valid()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  usb_.GetDeviceDescriptor(&device_desc_);

  auto configuration = usb_.GetConfiguration();
  size_t desc_length;
  auto status = usb_.GetConfigurationDescriptorLength(configuration, &desc_length);
  if (status != ZX_OK) {
    return status;
  }

  fbl::AllocChecker ac;
  auto desc_bytes = new (&ac) uint8_t[desc_length];
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  config_desc_.reset(desc_bytes, desc_length);

  size_t actual;
  status = usb_.GetConfigurationDescriptor(configuration, desc_bytes, desc_length, &actual);
  if (status == ZX_OK && actual != desc_length) {
    status = ZX_ERR_IO;
  }
  if (status != ZX_OK) {
    return status;
  }

  char name[16];
  snprintf(name, sizeof(name), "%03d", usb_.GetDeviceId());

  return DdkAdd(name, DEVICE_ADD_NON_BINDABLE);
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = UsbComposite::Create;
  return ops;
}();

}  // namespace usb_composite

ZIRCON_DRIVER(usb_composite, usb_composite::driver_ops, "zircon", "0.1");
