// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ti-tca6408a.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

namespace {

// Arbitrary values for I2C retries.
constexpr uint8_t kI2cRetries = 10;
constexpr zx::duration kI2cRetryDelay = zx::usec(1);

}  // namespace

namespace gpio {

zx::result<> TiTca6408aDevice::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  // Get I2C.
  ddk::I2cChannel i2c;
  {
    zx::result result = incoming->Connect<fuchsia_hardware_i2c::Service::Device>("i2c");
    if (result.is_error()) {
      fdf::error("Failed to open i2c service: {}", result);
      return result.take_error();
    }
    i2c = std::move(result.value());

    // Clear the polarity inversion register.
    const uint8_t write_buf[2] = {static_cast<uint8_t>(TiTca6408a::Register::kPolarityInversion),
                                  0};
    i2c.WriteSyncRetries(write_buf, sizeof(write_buf), kI2cRetries, kI2cRetryDelay);
  }

  device_ = std::make_unique<TiTca6408a>(std::move(i2c));

  auto result = outgoing()->AddService<fuchsia_hardware_pinimpl::Service>(
      fuchsia_hardware_pinimpl::Service::InstanceHandler({
          .device = bindings_.CreateHandler(device_.get(), fdf::Dispatcher::GetCurrent()->get(),
                                            fidl::kIgnoreBindingClosure),
      }));
  if (result.is_error()) {
    fdf::error("Failed to add Device service {}", result);
    return result.take_error();
  }

  zx::result pdev = incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev);
    return pdev.take_error();
  }

  if (zx::result result =
          pin_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev.value());
      result.is_error()) {
    fdf::error("Failed to set pin metadata from platform device: {}", result);
    return result.take_error();
  }

  if (zx::result result = scheduler_role_name_metadata_server_.ForwardAndServe(
          *outgoing(), dispatcher(), pdev.value());
      result.is_error()) {
    fdf::error("Failed to set scheduler role name metadata from platform device: {}", result);
    return result.take_error();
  }

  return CreateNode();
}

TiTca6408aDevice::~TiTca6408aDevice() {
  auto status = controller_->Remove();
  if (!status.ok()) {
    fdf::error("Could not remove child: {}", status.status_string());
  }
}

zx::result<> TiTca6408aDevice::CreateNode() {
  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_pinimpl::Service>()};
  std::optional pin_metadata_offer = pin_metadata_server_.CreateOffer();
  if (pin_metadata_offer.has_value()) {
    offers.push_back(std::move(pin_metadata_offer.value()));
  }
  std::optional scheduler_role_name_offer = scheduler_role_name_metadata_server_.CreateOffer();
  if (scheduler_role_name_offer.has_value()) {
    offers.push_back(std::move(scheduler_role_name_offer.value()));
  }

  zx::result child =
      AddChild(kDeviceName, std::vector<fuchsia_driver_framework::NodeProperty>{}, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  controller_.Bind(std::move(child.value()));

  return zx::ok();
}

void TiTca6408a::Read(ReadRequest& request, ReadCompleter::Sync& completer) {
  if (!IsIndexInRange(request.pin())) {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
    return;
  }

  zx::result<uint8_t> value = ReadBit(Register::kInputPort, request.pin());
  if (value.is_error()) {
    completer.Reply(zx::error(value.error_value()));
    return;
  }

  completer.Reply(fit::ok(value.value()));
}

void TiTca6408a::SetBufferMode(SetBufferModeRequest& request,
                               SetBufferModeCompleter::Sync& completer) {
  if (!IsIndexInRange(request.pin())) {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
    return;
  }

  if (request.mode() == fuchsia_hardware_gpio::BufferMode::kInput) {
    completer.Reply(SetBit(Register::kConfiguration, request.pin()));
    return;
  }

  zx::result<> status = request.mode() == fuchsia_hardware_gpio::BufferMode::kOutputHigh
                            ? SetBit(Register::kOutputPort, request.pin())
                            : ClearBit(Register::kOutputPort, request.pin());
  if (status.is_error()) {
    completer.Reply(status);
    return;
  }

  completer.Reply(ClearBit(Register::kConfiguration, request.pin()));
}

void TiTca6408a::GetInterrupt(GetInterruptRequest& request,
                              GetInterruptCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void TiTca6408a::ConfigureInterrupt(ConfigureInterruptRequest& request,
                                    ConfigureInterruptCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void TiTca6408a::ReleaseInterrupt(ReleaseInterruptRequest& request,
                                  ReleaseInterruptCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void TiTca6408a::Configure(ConfigureRequest& request, ConfigureCompleter::Sync& completer) {
  if (request.config().function() || request.config().drive_strength_ua()) {
    return completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  if (request.config().pull() && *request.config().pull() != fuchsia_hardware_pin::Pull::kNone) {
    return completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  completer.Reply(zx::ok(request.config()));
}

zx::result<uint8_t> TiTca6408a::ReadBit(Register reg, uint32_t index) {
  const auto bit = static_cast<uint8_t>(1 << index);
  const auto address = static_cast<uint8_t>(reg);

  uint8_t value = 0;
  auto status = i2c_.WriteReadSyncRetries(&address, sizeof(address), &value, sizeof(value),
                                          kI2cRetries, kI2cRetryDelay);
  if (status.status != ZX_OK) {
    fdf::error("Failed to read register {}: {}", address, zx_status_get_string(status.status));
    return zx::error(status.status);
  }

  return zx::ok(static_cast<uint8_t>((value & bit) ? 1 : 0));
}

zx::result<> TiTca6408a::SetBit(Register reg, uint32_t index) {
  const auto bit = static_cast<uint8_t>(1 << index);
  const auto address = static_cast<uint8_t>(reg);

  uint8_t value = 0;
  auto status = i2c_.WriteReadSyncRetries(&address, sizeof(address), &value, sizeof(value),
                                          kI2cRetries, kI2cRetryDelay);
  if (status.status != ZX_OK) {
    fdf::error("Failed to read register {}: {}", address, zx_status_get_string(status.status));
    return zx::error(status.status);
  }

  const uint8_t write_buf[2] = {address, static_cast<uint8_t>(value | bit)};
  status = i2c_.WriteSyncRetries(write_buf, sizeof(write_buf), kI2cRetries, kI2cRetryDelay);
  if (status.status != ZX_OK) {
    fdf::error("Failed to write register {}: {}", address, zx_status_get_string(status.status));
    return zx::error(status.status);
  }

  return zx::ok();
}

zx::result<> TiTca6408a::ClearBit(Register reg, uint32_t index) {
  const auto bit = static_cast<uint8_t>(1 << index);
  const auto address = static_cast<uint8_t>(reg);

  uint8_t value = 0;
  auto status = i2c_.WriteReadSyncRetries(&address, sizeof(address), &value, sizeof(value),
                                          kI2cRetries, kI2cRetryDelay);
  if (status.status != ZX_OK) {
    fdf::error("Failed to read register {}: {}", address, zx_status_get_string(status.status));
    return zx::error(status.status);
  }

  const uint8_t write_buf[2] = {address, static_cast<uint8_t>(value & ~bit)};
  status = i2c_.WriteSyncRetries(write_buf, sizeof(write_buf), kI2cRetries, kI2cRetryDelay);
  if (status.status != ZX_OK) {
    fdf::error("Failed to write register {}: {}", address, zx_status_get_string(status.status));
    return zx::error(status.status);
  }

  return zx::ok();
}

}  // namespace gpio

FUCHSIA_DRIVER_EXPORT2(gpio::TiTca6408aDevice);
