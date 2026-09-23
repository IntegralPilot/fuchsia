// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "serial.h"

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include <format>
#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/serial/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "src/devices/serial/drivers/serial/serial_config.h"

namespace serial {

void SerialDevice::Read(ReadCompleter::Sync& completer) {
  logger().log(fdf::TRACE, "SerialDevice::Read");

  fdf::Arena arena('SERI');
  serial_.buffer(arena)->Read().Then([completer = completer.ToAsync()](auto& result) mutable {
    if (!result.ok()) {
      completer.ReplyError(result.status());
    } else if (result->is_error()) {
      completer.ReplyError(result->error_value());
    } else {
      completer.ReplySuccess(result->value()->data);
    }
  });
}

void SerialDevice::Write(WriteRequestView request, WriteCompleter::Sync& completer) {
  fdf::Arena arena('SERI');
  serial_.buffer(arena)
      ->Write(request->data)
      .Then([completer = completer.ToAsync()](auto& result) mutable {
        if (!result.ok()) {
          completer.ReplyError(result.status());
        } else if (result->is_error()) {
          completer.ReplyError(result->error_value());
        } else {
          completer.ReplySuccess();
        }
      });
}

void SerialDevice::GetChannel(GetChannelRequestView request, GetChannelCompleter::Sync& completer) {
  logger().log(fdf::TRACE, "SerialDevice::GetChannel");
  if (zx_status_t status = Bind(std::move(request->req)); status != ZX_OK) {
    logger().log(fdf::ERROR, "SerialDevice::GetChannel error: {}", zx_status_get_string(status));
    completer.Close(status);
  }
}

void SerialDevice::GetClass(GetClassCompleter::Sync& completer) {
  logger().log(fdf::TRACE, "SerialDevice::GetClass");
  completer.Reply(static_cast<fuchsia_hardware_serial::wire::Class>(serial_class_));
}

void SerialDevice::SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) {
  using fuchsia_hardware_serial::wire::CharacterWidth;
  using fuchsia_hardware_serial::wire::FlowControl;
  using fuchsia_hardware_serial::wire::Parity;
  using fuchsia_hardware_serial::wire::StopWidth;
  uint32_t flags = 0;
  switch (request->config.character_width) {
    case CharacterWidth::kBits5:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialDataBits5;
      break;
    case CharacterWidth::kBits6:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialDataBits6;
      break;
    case CharacterWidth::kBits7:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialDataBits7;
      break;
    case CharacterWidth::kBits8:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialDataBits8;
      break;
  }

  switch (request->config.stop_width) {
    case StopWidth::kBits1:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialStopBits1;
      break;
    case StopWidth::kBits2:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialStopBits2;
      break;
  }

  switch (request->config.parity) {
    case Parity::kNone:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialParityNone;
      break;
    case Parity::kEven:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialParityEven;
      break;
    case Parity::kOdd:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialParityOdd;
      break;
  }

  switch (request->config.control_flow) {
    case FlowControl::kNone:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialFlowCtrlNone;
      break;
    case FlowControl::kCtsRts:
      flags |= fuchsia_hardware_serialimpl::wire::kSerialFlowCtrlCtsRts;
      break;
  }

  fdf::Arena arena('SERI');
  serial_.buffer(arena)
      ->Config(request->config.baud_rate, flags)
      .Then([completer = completer.ToAsync()](auto& result) mutable {
        if (result.ok()) {
          completer.Reply(result->is_error() ? result->error_value() : ZX_OK);
        } else {
          completer.Reply(result.status());
        }
      });
}

zx_status_t SerialDevice::Enable(bool enable) {
  fdf::Arena arena('SERI');
  auto result = serial_.sync().buffer(arena)->Enable(enable);
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

void SerialDevice::ResetSerialImplConnectionAndThen(fit::closure completer) {
  if (!serial_.is_valid()) {
    completer();
    return;
  }

  fdf::Arena arena('SERI');
  serial_.buffer(arena)->CancelAll().Then([this, &serial = serial_, arena = std::move(arena),
                                           completer = std::move(completer)](auto&) mutable {
    // Explicitly ignoring the result of CancelAll.
    logger().log(fdf::TRACE,
                 "SerialDevice::ResetSerialImplConnectionAndThen - pending operations aborted");
    serial.buffer(arena)->Enable(false).Then([this,
                                              completer = std::move(completer)](auto&) mutable {
      logger().log(fdf::TRACE, "SerialDevice::ResetSerialImplConnectionAndThen - disabled serial");
      // Explicitly ignoring the result of Enable.
      completer();
    });
  });
}

zx_status_t SerialDevice::Bind(fidl::ServerEnd<fuchsia_hardware_serial::Device> server) {
  if (!serial_.is_valid()) {
    // Use the first Zircon transport connection attempt as a signal that our client wants to use it
    // instead of driver transport. We can keep the connection with our parent open once the client
    // has indicated this.
    zx::result serial_client = incoming()->Connect<fuchsia_hardware_serialimpl::Service::Device>();
    if (serial_client.is_error()) {
      logger().log(fdf::ERROR, "Failed to get FIDL serial client: {}", serial_client);
      return serial_client.error_value();
    }

    serial_.Bind(*std::move(serial_client), fdf::Dispatcher::GetCurrent()->get());
  }

  if (binding_.has_value()) {
    logger().log(fdf::WARN, "SerialDevice::Bind - already bound!");
    return ZX_ERR_ALREADY_BOUND;
  }

  if (zx_status_t status = Enable(true); status != ZX_OK) {
    return status;
  }

  binding_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server), this,
                   [](SerialDevice* self, fidl::UnbindInfo) {
                     self->logger().log(fdf::TRACE, "SerialDevice::Bind - on close");
                     self->ResetSerialImplConnectionAndThen([self]() { self->binding_.reset(); });
                   });
  return ZX_OK;
}

void SerialDevice::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_serial::DeviceProxy> server) {
  logger().log(fdf::TRACE, "SerialDevice::DevfsConnect");

  proxy_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server),
                             this, fidl::kIgnoreBindingClosure);
}

void SerialDevice::Stop(fdf::StopCompleter completer) {
  logger().log(fdf::TRACE, "SerialDevice::Stop");
  ResetSerialImplConnectionAndThen([this, completer = std::move(completer)]() mutable {
    logger().log(fdf::TRACE, "SerialDevice::Stop - completed");
    completer(zx::ok());
  });
}

zx::result<> SerialDevice::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  logger().log(fdf::TRACE, "SerialDevice::Start");

  if (zx_status_t status = Init(); status != ZX_OK) {
    return zx::error(status);
  }

  const auto config = context.take_config<serial_config::Config>();
  if (zx_status_t status = Bind(config); status != ZX_OK) {
    logger().log(fdf::ERROR, "SerialDevice::Create: Bind failed {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok();
}

zx_status_t SerialDevice::Init() {
  zx::result serial_client = incoming()->Connect<fuchsia_hardware_serialimpl::Service::Device>();
  if (serial_client.is_error()) {
    logger().log(fdf::ERROR, "Failed to get FIDL serial client: {}", serial_client);
    return serial_client.error_value();
  }

  fdf::WireSyncClient client(*std::move(serial_client));

  zx_status_t status = ZX_OK;
  fdf::Arena arena('SERI');
  if (auto result = client.buffer(arena)->GetInfo(); !result.ok()) {
    status = result.status();
  } else if (result->is_error()) {
    status = result->error_value();
  } else {
    serial_class_ = static_cast<uint8_t>(result->value()->info.serial_class);
  }

  if (status != ZX_OK) {
    logger().log(fdf::ERROR, "SerialDevice::Init: SerialImpl::GetInfo failed {}",
                 zx_status_get_string(status));
  }

  return status;
}

zx_status_t SerialDevice::Bind(serial_config::Config config) {
  {
    fuchsia_hardware_serial::Service::InstanceHandler handler({
        .device =
            [this](fidl::ServerEnd<fuchsia_hardware_serial::Device> server) {
              Bind(std::move(server));
            },
    });
    zx::result<> result =
        outgoing()->AddService<fuchsia_hardware_serial::Service>(std::move(handler));
    if (result.is_error()) {
      logger().log(fdf::ERROR, "Failed to add service to the outgoing directory: {}", result);
      return result.error_value();
    }
  }

  {
    // Forward driver transport connection attempts to the parent.
    fuchsia_hardware_serialimpl::Service::InstanceHandler handler({
        .device =
            [this](fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server) {
              zx::result<> result =
                  incoming()->Connect<fuchsia_hardware_serialimpl::Service::Device>(
                      std::move(server));
              if (result.is_error()) {
                logger().log(fdf::WARN, "Failed to connect to serialimpl service: {}", result);
              }
            },
    });
    if (zx::result<> result =
            outgoing()->AddService<fuchsia_hardware_serialimpl::Service>(std::move(handler));
        result.is_error()) {
      logger().log(fdf::ERROR, "Failed to add service: {}", result);
      return result.error_value();
    }
  }

  std::vector<fuchsia_driver_framework::Offer> serial_offers{
      fdf::MakeOffer2<fuchsia_hardware_serial::Service>(),
      fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(),
  };

  if (config.enable_suspend()) {
    // Forward PowerTokenProvider to the parent if suspend is enabled.
    fuchsia_hardware_power::PowerTokenService::InstanceHandler handler({
        .token_provider =
            [this](fidl::ServerEnd<fuchsia_hardware_power::PowerTokenProvider> server) {
              zx::result<> result =
                  incoming()->Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
                      std::move(server));
              if (result.is_error()) {
                logger().log(fdf::WARN, "Failed to connect to power token service: {}", result);
              }
            },
    });
    if (zx::result<> result =
            outgoing()->AddService<fuchsia_hardware_power::PowerTokenService>(std::move(handler));
        result.is_error()) {
      logger().log(fdf::ERROR, "Failed to add power token service: {}", result);
      return result.error_value();
    }

    serial_offers.push_back(fdf::MakeOffer2<fuchsia_hardware_power::PowerTokenService>());
  }

  // Forward MAC address metadata if it exists.
  if (zx::result<bool> result =
          mac_address_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming());
      result.is_error()) {
    logger().log(fdf::ERROR, "Failed to forward metadata: {}", result);
    return result.error_value();
  }

  logger().log(fdf::TRACE, "SerialDevice added service to the outgoing directory");

  const std::optional mac_address_offer = mac_address_metadata_server_.CreateOffer();
  if (mac_address_offer.has_value()) {
    serial_offers.push_back(mac_address_offer.value());
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> legacy_props{
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_serial::BIND_PROTOCOL_DEVICE),
      fdf::MakeProperty2(bind_fuchsia::SERIAL_CLASS, serial_class_),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.serial.Service"),
  };

  // 1. Add the legacy serial node (offers serial::Service)
  zx::result<fidl::ClientEnd<fuchsia_device_fs::Connector>> connector =
      devfs_connector_.Bind(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  if (connector.is_error()) {
    logger().log(fdf::ERROR, "Failed to bind devfs connector: {}", connector);
    return connector.error_value();
  }

  logger().log(fdf::TRACE, "SerialDevice bound devfs connector");

  fuchsia_driver_framework::DevfsAddArgs devfs{{
      .connector = *std::move(connector),
      .class_name = "serial",
      .connector_supports = fuchsia_device_fs::ConnectionType::kDevice,
  }};

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> controller =
      fdf::AddChild(node(), logger(), name(), devfs, legacy_props, serial_offers);
  if (controller.is_error()) {
    logger().log(fdf::ERROR, "AddChild failed: {}", controller);
    return controller.error_value();
  }

  logger().log(fdf::TRACE, "SerialDevice registered devfs node: {}", name());
  controller_ = *std::move(controller);

  // 2. Add the new serial-impl node (offers serialimpl::Service)
  std::vector<fuchsia_driver_framework::Offer> impl_offers{
      fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> impl_props{
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_serial::BIND_PROTOCOL_DEVICE),
      fdf::MakeProperty2(bind_fuchsia::SERIAL_CLASS, serial_class_),
  };

  std::string impl_node_name = std::format("{}-impl", name());
  auto impl_result = fdf::AddChild(node(), logger(), impl_node_name, impl_props, impl_offers);
  if (impl_result.is_error()) {
    logger().log(fdf::ERROR, "Failed to add serial-impl child node: {}", impl_result);
    return impl_result.error_value();
  }
  impl_controller_ = std::move(impl_result.value());

  return ZX_OK;
}

}  // namespace serial

FUCHSIA_DRIVER_EXPORT2(serial::SerialDevice);
