// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gpio.h"

#include <fidl/fuchsia.driver.metadata/cpp/wire.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/node/cpp/add_child.h>
#include <lib/fit/defer.h>
#include <zircon/types.h>

#include <algorithm>
#include <format>
#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "src/devices/gpio/drivers/gpio/gpio_parser.h"

namespace gpio {

void GpioDevice::GpioInstance::Read(ReadCompleter::Sync& completer) {
  fdf::Arena arena('GPIO');
  pinimpl_.buffer(arena)->Read(pin_).ThenExactlyOnce(
      fit::inline_callback<void(fdf::WireUnownedResult<fuchsia_hardware_pinimpl::PinImpl::Read>&),
                           sizeof(ReadCompleter::Async)>(
          [completer = completer.ToAsync()](auto& result) mutable {
            if (!result.ok()) {
              completer.ReplyError(result.status());
            } else if (result->is_error()) {
              completer.ReplyError(result->error_value());
            } else {
              completer.ReplySuccess(result->value()->value);
            }
          }));
}

void GpioDevice::GpioInstance::SetBufferMode(SetBufferModeRequestView request,
                                             SetBufferModeCompleter::Sync& completer) {
  fdf::Arena arena('GPIO');
  pinimpl_.buffer(arena)
      ->SetBufferMode(pin_, request->mode)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_pinimpl::PinImpl::SetBufferMode>&),
              sizeof(SetBufferModeCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (result.ok()) {
                  completer.Reply(*result);
                } else {
                  completer.ReplyError(result.status());
                }
              }));
}

void GpioDevice::GpioInstance::GetInterrupt(GetInterruptRequestView request,
                                            GetInterruptCompleter::Sync& completer) {
  if (has_interrupt()) {
    completer.ReplyError(ZX_ERR_ALREADY_EXISTS);
    return;
  }
  if (parent_->gpio_instance_has_interrupt()) {
    completer.ReplyError(ZX_ERR_ACCESS_DENIED);
    return;
  }

  interrupt_state_ = InterruptState::kGettingInterrupt;

  fdf::Arena arena('GPIO');
  pinimpl_.buffer(arena)
      ->GetInterrupt(pin_, request->options)
      .ThenExactlyOnce(
          [instance = fbl::RefPtr(this), completer = completer.ToAsync()](auto& result) mutable {
            // Clear ownership of the interrupt if the call failed.
            if (!result.ok()) {
              instance->interrupt_state_ = InterruptState::kNoInterrupt;
              completer.ReplyError(result.status());
            } else if (result->is_error()) {
              instance->interrupt_state_ = InterruptState::kNoInterrupt;
              completer.ReplyError(result->error_value());
            } else {
              instance->interrupt_state_ = InterruptState::kHasInterrupt;
              completer.ReplySuccess(std::move(result->value()->interrupt));
            }

            if (instance->release_instance_after_call_completes_) {
              instance->ReleaseInstance();
            }
          });
}

void GpioDevice::GpioInstance::ConfigureInterrupt(
    fuchsia_hardware_gpio::wire::GpioConfigureInterruptRequest* request,
    ConfigureInterruptCompleter::Sync& completer) {
  if (parent_->gpio_instance_has_interrupt() && !has_interrupt()) {
    // Allow the interrupt to be configured if we own it, or if no instance has one.
    completer.ReplyError(ZX_ERR_ACCESS_DENIED);
    return;
  }

  fdf::Arena arena('GPIO');
  pinimpl_.buffer(arena)
      ->ConfigureInterrupt(pin_, request->config)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_pinimpl::PinImpl::ConfigureInterrupt>&),
              sizeof(ConfigureInterruptCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (result.ok()) {
                  completer.Reply(*result);
                } else {
                  completer.ReplyError(result.status());
                }
              }));
}

void GpioDevice::GpioInstance::ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) {
  if (parent_->gpio_instance_has_interrupt() && !has_interrupt()) {
    completer.ReplyError(ZX_ERR_ACCESS_DENIED);
    return;
  }
  if (interrupt_state_ != InterruptState::kHasInterrupt) {
    // We might be in the process of getting the interrupt now, but we haven't returned it to the
    // client yet.
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  interrupt_state_ = InterruptState::kReleasingInterrupt;

  fdf::Arena arena('GPIO');
  pinimpl_.buffer(arena)->ReleaseInterrupt(pin_).ThenExactlyOnce(
      [instance = fbl::RefPtr(this), completer = completer.ToAsync()](auto& result) mutable {
        if (result.ok()) {
          completer.Reply(*result);
        } else {
          completer.ReplyError(result.status());
        }

        instance->interrupt_state_ = InterruptState::kNoInterrupt;
        if (instance->release_instance_after_call_completes_) {
          instance->ReleaseInstance();
        }
      });
}

void GpioDevice::GpioInstance::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  parent_->logger().log(fdf::ERROR, "Unknown Gpio method ordinal 0x{:016x}",
                        metadata.method_ordinal);
}

void GpioDevice::GpioInstance::OnUnbound(fidl::UnbindInfo info) {
  if (interrupt_state_ == InterruptState::kHasInterrupt ||
      interrupt_state_ == InterruptState::kNoInterrupt) {
    // There are no calls pending, so release the interrupt if there is one, then tell the parent to
    // release us.
    ReleaseInstance();
  } else {
    // A call is pending -- set release_instance_after_call_completes_ and wait for the call to
    // complete.
    release_instance_after_call_completes_ = true;
  }
}

void GpioDevice::GpioInstance::ReleaseInstance() {
  release_instance_after_call_completes_ = false;
  if (interrupt_state_ == InterruptState::kHasInterrupt) {
    interrupt_state_ = InterruptState::kReleasingInterrupt;

    fdf::Arena arena('GPIO');
    pinimpl_.buffer(arena)->ReleaseInterrupt(pin_).Then(
        [instance = fbl::RefPtr(this)](auto& result) {
          instance->interrupt_state_ = InterruptState::kNoInterrupt;
          instance->RemoveFromContainer();
        });
  } else if (interrupt_state_ == InterruptState::kNoInterrupt) {
    RemoveFromContainer();
  } else {
    ZX_DEBUG_ASSERT_MSG(false, "ReleaseInstance called in an invalid state");
  }
}

bool GpioDevice::gpio_instance_has_interrupt() const {
  return std::any_of(gpio_instances_.cbegin(), gpio_instances_.cend(),
                     [](const GpioInstance& instance) { return instance.has_interrupt(); });
}

void GpioDevice::Configure(fuchsia_hardware_pin::wire::PinConfigureRequest* request,
                           ConfigureCompleter::Sync& completer) {
  fdf::Arena arena('GPIO');
  pinimpl_.buffer(arena)
      ->Configure(pin_, request->config)
      .ThenExactlyOnce(fit::inline_callback<
                       void(fdf::WireUnownedResult<fuchsia_hardware_pinimpl::PinImpl::Configure>&),
                       sizeof(ConfigureCompleter::Async)>(
          [completer = completer.ToAsync()](auto& result) mutable {
            if (!result.ok()) {
              completer.ReplyError(result.status());
            } else if (result->is_error()) {
              completer.ReplyError(result->error_value());
            } else {
              completer.ReplySuccess(result->value()->new_config);
            }
          }));
}

void GpioDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Pin> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  logger().log(fdf::ERROR, "Unknown Pin method ordinal: 0x{:016x}", metadata.method_ordinal);
}

void GpioDevice::GetProperties(GetPropertiesCompleter::Sync& completer) {
  fdf::Arena arena('GPIO');
  auto properties = fuchsia_hardware_pin::wire::DebugGetPropertiesResponse::Builder(arena)
                        .name(fidl::StringView::FromExternal(name_))
                        .pin(pin_)
                        .Build();
  completer.Reply(properties);
}

void GpioDevice::ConnectPin(fuchsia_hardware_pin::wire::DebugConnectPinRequest* request,
                            ConnectPinCompleter::Sync& completer) {
  pin_bindings_.AddBinding(fidl_dispatcher_, std::move(request->server), this,
                           fidl::kIgnoreBindingClosure);
  completer.ReplySuccess();
}

void GpioDevice::ConnectGpio(fuchsia_hardware_pin::wire::DebugConnectGpioRequest* request,
                             ConnectGpioCompleter::Sync& completer) {
  ConnectGpio(std::move(request->server));
  completer.ReplySuccess();
}

void GpioDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Debug> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  logger().log(fdf::ERROR, "Unknown Debug method ordinal: 0x{:016x}", metadata.method_ordinal);
}

void GpioDevice::ConnectGpio(fidl::ServerEnd<fuchsia_hardware_gpio::Gpio> server) {
  gpio_instances_.push_front(fbl::MakeRefCounted<GpioInstance>(fidl_dispatcher_, std::move(server),
                                                               pinimpl_.Clone(), pin_, this));
}

zx::result<> GpioDevice::AddServices(const std::shared_ptr<fdf::Namespace>& incoming,
                                     const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                                     gpio_config::Config config) {
  fuchsia_hardware_gpio::Service::InstanceHandler gpio_handler({
      .device =
          [&](fidl::ServerEnd<fuchsia_hardware_gpio::Gpio> server) {
            async::PostTask(fidl_dispatcher_, [this, server = std::move(server)]() mutable {
              ConnectGpio(std::move(server));
            });
          },
  });
  zx::result<> service_result =
      outgoing->AddService<fuchsia_hardware_gpio::Service>(std::move(gpio_handler), pin_name());
  if (service_result.is_error()) {
    logger().log(fdf::ERROR, "Failed to add Gpio service to the outgoing directory");
    return service_result.take_error();
  }

  fuchsia_hardware_pin::Service::InstanceHandler pin_handler({
      .device =
          [&](fidl::ServerEnd<fuchsia_hardware_pin::Pin> server) {
            async::PostTask(fidl_dispatcher_, [this, server = std::move(server)]() mutable {
              pin_bindings_.AddBinding(fidl_dispatcher_, std::move(server), this,
                                       fidl::kIgnoreBindingClosure);
            });
          },
  });
  service_result =
      outgoing->AddService<fuchsia_hardware_pin::Service>(std::move(pin_handler), pin_name());
  if (service_result.is_error()) {
    logger().log(fdf::ERROR, "Failed to add Pin service to the outgoing directory");
    return service_result.take_error();
  }

  if (config.enable_suspend()) {
    // Forward PowerTokenService to our parent if suspend is enabled.
    fuchsia_hardware_power::PowerTokenService::InstanceHandler handler({
        .token_provider =
            [this, incoming = incoming](
                fidl::ServerEnd<fuchsia_hardware_power::PowerTokenProvider> server) {
              zx::result<> result =
                  incoming->Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
                      std::move(server));
              if (result.is_error()) {
                logger().log(fdf::WARN, "Failed to connect to power token service: {}", result);
              }
            },
    });

    zx::result result = outgoing->AddService<fuchsia_hardware_power::PowerTokenService>(
        std::move(handler), pin_name());
    if (result.is_error()) {
      logger().log(fdf::ERROR, "Failed to add power token service: {}", result);
      return result.take_error();
    }
  }

  return zx::ok();
}

zx::result<> GpioDevice::AddDevice(fidl::UnownedClientEnd<fuchsia_driver_framework::Node> root_node,
                                   fdf::Logger& logger, gpio_config::Config config) {
  // 1. Add the legacy GPIO node (offers gpio::Service and PowerTokenService)
  std::vector<fuchsia_driver_framework::Offer> gpio_offers{
      fdf::MakeOffer2<fuchsia_hardware_gpio::Service>(pin_name()),
      fdf::MakeOffer2<fuchsia_hardware_pin::Service>(pin_name()),
  };

  if (config.enable_suspend()) {
    gpio_offers.emplace_back(
        fdf::MakeOffer2<fuchsia_hardware_power::PowerTokenService>(pin_name()));
  }

  const uint32_t id = id_.value_or(pin_);
  std::vector<fuchsia_driver_framework::NodeProperty2> gpio_props{
      fdf::MakeProperty2(bind_fuchsia::ID, id),
      fdf::MakeProperty2(bind_fuchsia::NAME, pin_name()),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> pin_props{
      fdf::MakeProperty2(bind_fuchsia::ID, id),
      fdf::MakeProperty2(bind_fuchsia::NAME, pin_name()),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pin.Service"),
  };

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> result;
  if (config.expose_debug_capabilities()) {
    zx::result connector = devfs_connector_.Bind(fidl_dispatcher_);
    if (connector.is_error()) {
      logger.log(fdf::ERROR, "Failed to bind devfs connector: {}", connector.status_string());
      return connector.take_error();
    }

    fuchsia_driver_framework::DevfsAddArgs devfs_args{{
        .connector = std::move(connector.value()),
        .class_name = "gpio",
    }};
    result = fdf::AddChild(root_node, logger, pin_name(), devfs_args, gpio_props, gpio_offers);
  } else {
    result = fdf::AddChild(root_node, logger, pin_name(), gpio_props, gpio_offers);
  }

  if (result.is_error()) {
    logger.log(fdf::ERROR, "Failed to add gpio child node: {}", result.status_string());
    return result.take_error();
  }
  controller_ = std::move(result.value());

  // 2. Add the new Pin node (offers pin::Service)
  std::vector<fuchsia_driver_framework::Offer> pin_offers{
      fdf::MakeOffer2<fuchsia_hardware_pin::Service>(pin_name()),
  };

  std::string pin_node_name = std::format("{}-pin", pin_name());
  auto pin_result = fdf::AddChild(root_node, logger, pin_node_name, pin_props, pin_offers);
  if (pin_result.is_error()) {
    logger.log(fdf::ERROR, "Failed to add pin child node: {}", pin_result.status_string());
    return pin_result.take_error();
  }
  pin_controller_ = std::move(pin_result.value());

  return zx::ok();
}

void GpioDevice::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_pin::Debug> server) {
  debug_bindings_.AddBinding(fidl_dispatcher_, std::move(server), this,
                             fidl::kIgnoreBindingClosure);
}

//
// PinStatesDevice implementation
//

zx::result<> PinStatesDevice::AddServices(const std::shared_ptr<fdf::Namespace>& incoming,
                                          const std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
  fuchsia_hardware_pin::PinStatesService::InstanceHandler pin_handler({
      .device =
          [&](fidl::ServerEnd<fuchsia_hardware_pin::PinStates> server) {
            async::PostTask(fidl_dispatcher_, [this, server = std::move(server)]() mutable {
              bindings_.AddBinding(fidl_dispatcher_, std::move(server), this,
                                   fidl::kIgnoreBindingClosure);
            });
          },
  });
  zx::result<> service_result = outgoing->AddService<fuchsia_hardware_pin::PinStatesService>(
      std::move(pin_handler), pin_states_.name());
  if (service_result.is_error()) {
    logger_.log(fdf::ERROR, "Failed to add PinStates service to the outgoing directory");
    return service_result.take_error();
  }
  return zx::ok();
}

zx::result<> PinStatesDevice::AddDevice(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> root_node) {
  std::vector<fuchsia_driver_framework::Offer> offers{
      fdf::MakeOffer2<fuchsia_hardware_pin::PinStatesService>(pin_states_.name()),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> props{
      fdf::MakeProperty2(bind_fuchsia::ID, controller_id_),
      fdf::MakeProperty2(bind_fuchsia::NAME, pin_states_.name()),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pin.PinStatesService"),
  };

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> result =
      fdf::AddChild(root_node, logger_, pin_states_.name(), props, offers);
  if (result.is_error()) {
    logger_.log(fdf::ERROR, "Failed to add pin-states child node: {}", result);
    return result.take_error();
  }

  controller_ = std::move(result.value());
  return zx::ok();
}

zx_status_t PinStatesDevice::ApplyState(const std::string& state_name) {
  const fuchsia_hardware_pinimpl::PinState* target_state = nullptr;
  for (const auto& state : pin_states_.states()) {
    if (state.name() == state_name) {
      target_state = &state;
      break;
    }
  }

  if (!target_state) {
    logger_.log(fdf::ERROR, "Pin state '{}' not found for device '{}'", state_name,
                pin_states_.name());
    return ZX_ERR_NOT_FOUND;
  }

  fdf::Arena arena('GPIO');
  for (const auto& pin_cfg : target_state->pins()) {
    uint32_t pin = pin_cfg.pin();

    if (pin_cfg.call().Which() == fuchsia_hardware_pinimpl::InitCall::Tag::kPinConfig) {
      auto result = pinimpl_.sync().buffer(arena)->Configure(
          pin, fidl::ToWire(arena, pin_cfg.call().pin_config().value()));
      if (!result.ok()) {
        logger_.log(fdf::ERROR, "Failed to configure pin {} for state '{}': {}", pin, state_name,
                    result.status_string());
        return result.status();
      }
      if (result->is_error()) {
        logger_.log(fdf::ERROR, "PinImpl Configure returned error for pin {} state '{}': {}", pin,
                    state_name, result->error_value());
        return result->error_value();
      }
    } else if (pin_cfg.call().Which() == fuchsia_hardware_pinimpl::InitCall::Tag::kBufferMode) {
      auto result =
          pinimpl_.sync().buffer(arena)->SetBufferMode(pin, pin_cfg.call().buffer_mode().value());
      if (!result.ok()) {
        logger_.log(fdf::ERROR, "Failed to set buffer mode for pin {} state '{}': {}", pin,
                    state_name, result.status_string());
        return result.status();
      }
      if (result->is_error()) {
        logger_.log(fdf::ERROR, "PinImpl SetBufferMode returned error for pin {} state '{}': {}",
                    pin, state_name, result->error_value());
        return result->error_value();
      }
    }
  }

  return ZX_OK;
}

zx::result<> PinStatesDevice::ApplyDefaultState() {
  zx_status_t status = ApplyState("default");
  if (status == ZX_ERR_NOT_FOUND) {
    logger_.log(fdf::INFO, "No 'default' pin state found for device '{}'", pin_states_.name());
    return zx::ok();
  }
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

void PinStatesDevice::SelectState(SelectStateRequestView request,
                                  SelectStateCompleter::Sync& completer) {
  zx_status_t status = ApplyState(std::string(request->name.data(), request->name.size()));
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess();
  }
}

void PinStatesDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_pin::PinStates> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  logger_.log(fdf::ERROR, "Unknown PinStates method ordinal 0x{:016x}", metadata.method_ordinal);
}

namespace {

std::optional<fuchsia_hardware_pin::Pull> ConvertPull(std::optional<gpio_metadata::Pull> pull) {
  if (!pull)
    return std::nullopt;
  switch (*pull) {
    case gpio_metadata::Pull::kNone:
      return fuchsia_hardware_pin::Pull::kNone;
    case gpio_metadata::Pull::kUp:
      return fuchsia_hardware_pin::Pull::kUp;
    case gpio_metadata::Pull::kDown:
      return fuchsia_hardware_pin::Pull::kDown;
  }
}

std::optional<fuchsia_hardware_gpio::BufferMode> ConvertBufferMode(
    std::optional<gpio_metadata::BufferMode> buffer_mode) {
  if (!buffer_mode)
    return std::nullopt;
  switch (*buffer_mode) {
    case gpio_metadata::BufferMode::kInput:
      return fuchsia_hardware_gpio::BufferMode::kInput;
    case gpio_metadata::BufferMode::kOutputLow:
      return fuchsia_hardware_gpio::BufferMode::kOutputLow;
    case gpio_metadata::BufferMode::kOutputHigh:
      return fuchsia_hardware_gpio::BufferMode::kOutputHigh;
  }
}

std::optional<fuchsia_hardware_pin::DriveType> ConvertDriveType(
    std::optional<gpio_metadata::DriveType> drive_type) {
  if (!drive_type)
    return std::nullopt;
  switch (*drive_type) {
    case gpio_metadata::DriveType::kPushPull:
      return fuchsia_hardware_pin::DriveType::kPushPull;
    case gpio_metadata::DriveType::kOpenDrain:
      return fuchsia_hardware_pin::DriveType::kOpenDrain;
    case gpio_metadata::DriveType::kOpenSource:
      return fuchsia_hardware_pin::DriveType::kOpenSource;
  }
}

std::optional<fuchsia_hardware_pinimpl::InitCall> ConvertInitCall(
    const gpio_metadata::InitCall& gc) {
  if (gc.pin_config &&
      (gc.pin_config->pull || gc.pin_config->function || gc.pin_config->function_name ||
       gc.pin_config->drive_strength_ua || gc.pin_config->drive_type)) {
    fuchsia_hardware_pin::Configuration config;
    if (auto pull = ConvertPull(gc.pin_config->pull)) {
      config.pull(*pull);
    }
    if (gc.pin_config->function) {
      config.function(*gc.pin_config->function);
    }
    if (gc.pin_config->function_name) {
      config.function_name(*gc.pin_config->function_name);
    }
    if (gc.pin_config->drive_strength_ua) {
      config.drive_strength_ua(*gc.pin_config->drive_strength_ua);
    }
    if (auto drive_type = ConvertDriveType(gc.pin_config->drive_type)) {
      config.drive_type(*drive_type);
    }
    return fuchsia_hardware_pinimpl::InitCall::WithPinConfig(std::move(config));
  } else if (auto mode = ConvertBufferMode(gc.buffer_mode)) {
    return fuchsia_hardware_pinimpl::InitCall::WithBufferMode(*mode);
  }
  return std::nullopt;
}

std::vector<fuchsia_hardware_pinimpl::PinConfiguration> ConvertPinConfigs(
    std::vector<gpio_metadata::PinConfiguration> gpcs) {
  std::vector<fuchsia_hardware_pinimpl::PinConfiguration> pcs;
  for (auto& gpc : gpcs) {
    if (auto init_call = ConvertInitCall(gpc.call)) {
      pcs.emplace_back(gpc.pin, std::move(*init_call));
    }
  }
  return pcs;
}

std::vector<fuchsia_hardware_pinimpl::PinState> ConvertPinStates(
    std::vector<gpio_metadata::PinState> gpss) {
  std::vector<fuchsia_hardware_pinimpl::PinState> pss;
  for (auto& gps : gpss) {
    fuchsia_hardware_pinimpl::PinState ps;
    ps.name(std::move(gps.name));
    ps.pins(ConvertPinConfigs(std::move(gps.pins)));
    pss.push_back(std::move(ps));
  }
  return pss;
}

std::vector<fuchsia_hardware_pinimpl::DevicePinStates> ConvertDevicePinStates(
    std::vector<gpio_metadata::DevicePinStates> gdps) {
  std::vector<fuchsia_hardware_pinimpl::DevicePinStates> dps;
  for (auto& gd : gdps) {
    fuchsia_hardware_pinimpl::DevicePinStates d;
    d.name(std::move(gd.name));
    d.states(ConvertPinStates(std::move(gd.states)));
    dps.push_back(std::move(d));
  }
  return dps;
}

std::optional<fuchsia_hardware_pinimpl::Metadata> ConvertMetadata(
    gpio_metadata::GpioMetadata generic) {
  fuchsia_hardware_pinimpl::Metadata metadata;
  metadata.controller_id(generic.controller_id);

  std::vector<fuchsia_hardware_pinimpl::Pin> pins;
  std::vector<fuchsia_hardware_pinimpl::InitStep> init_steps;

  for (auto& p : generic.pins) {
    fuchsia_hardware_pinimpl::Pin pin;
    pin.pin(p.pin);
    if (p.name) {
      pin.name(std::move(*p.name));
    }
    if (p.id) {
      pin.id(*p.id);
    }
    pins.push_back(std::move(pin));

    // Generate InitSteps
    fuchsia_hardware_pin::Configuration config;
    bool has_config = false;

    if (p.function) {
      config.function(*p.function);
      has_config = true;
    }
    if (p.function_name) {
      config.function_name(*p.function_name);
      has_config = true;
    }
    if (p.drive_strength_ua) {
      config.drive_strength_ua(*p.drive_strength_ua);
      has_config = true;
    }
    if (auto pull = ConvertPull(p.pull)) {
      config.pull(*pull);
      has_config = true;
    }
    if (auto drive_type = ConvertDriveType(p.drive_type)) {
      config.drive_type(*drive_type);
      has_config = true;
    }

    if (has_config) {
      fuchsia_hardware_pinimpl::InitCall init_call =
          fuchsia_hardware_pinimpl::InitCall::WithPinConfig(std::move(config));
      fuchsia_hardware_pinimpl::InitStep step = fuchsia_hardware_pinimpl::InitStep::WithCall(
          fuchsia_hardware_pinimpl::Call(p.pin, std::move(init_call)));
      init_steps.push_back(std::move(step));
    }

    if (auto mode = ConvertBufferMode(p.buffer_mode)) {
      fuchsia_hardware_pinimpl::InitCall init_call =
          fuchsia_hardware_pinimpl::InitCall::WithBufferMode(*mode);
      fuchsia_hardware_pinimpl::InitStep step = fuchsia_hardware_pinimpl::InitStep::WithCall(
          fuchsia_hardware_pinimpl::Call(p.pin, std::move(init_call)));
      init_steps.push_back(std::move(step));
    }
  }

  metadata.pins(std::move(pins));
  metadata.init_steps(std::move(init_steps));

  if (generic.device_pin_states) {
    metadata.device_pin_states(ConvertDevicePinStates(std::move(*generic.device_pin_states)));
  }

  return metadata;
}

}  // namespace

std::optional<fuchsia_hardware_pinimpl::Metadata> GpioRootDevice::GetGenericMetadata() {
  auto client_end = fdf_metadata::ConnectToMetadataProtocol(incoming()->svc_dir(),
                                                            "fuchsia.hardware.pinimpl.Metadata");
  if (client_end.is_error()) {
    return std::nullopt;
  }
  fidl::WireSyncClient<fuchsia_driver_metadata::Metadata> client(std::move(client_end.value()));
  auto persisted_res = client->GetPersistedMetadata();
  if (!persisted_res.ok()) {
    logger().log(fdf::INFO, "Failed to send GetPersistedMetadata request: {}",
                 persisted_res.status_string());
    return std::nullopt;
  }
  if (persisted_res->is_error()) {
    logger().log(fdf::INFO, "GetPersistedMetadata returned error: {}",
                 zx_status_get_string(persisted_res->error_value()));
    return std::nullopt;
  }

  auto unpersisted = fidl::Unpersist<fuchsia_driver_metadata::Dictionary>(
      persisted_res.value()->persisted_metadata.get());
  if (unpersisted.is_error()) {
    logger().log(fdf::INFO, "Failed to unpersist as Dictionary (might be old metadata): {}",
                 unpersisted.error_value().status_string());
    return std::nullopt;
  }

  const auto& dict = unpersisted.value();
  if (!dict.entries().has_value() || dict.entries()->empty()) {
    logger().log(fdf::INFO, "Generic GPIO metadata dictionary is empty");
    return std::nullopt;
  }

  auto parsed = gpio_metadata::GpioMetadata::Parse(dict);
  if (!parsed) {
    logger().log(fdf::ERROR, "Failed to parse generic GPIO metadata");
    return std::nullopt;
  }

  return ConvertMetadata(std::move(*parsed));
}

std::optional<fuchsia_hardware_pinimpl::Metadata> GpioRootDevice::GetMetadata() {
  if (auto metadata = GetGenericMetadata()) {
    return metadata;
  }

  // Fall back to old metadata
  zx::result result =
      fdf_metadata::GetMetadataIfExists<fuchsia_hardware_pinimpl::Metadata>(incoming());
  if (result.is_error()) {
    logger().log(fdf::ERROR, "Failed to get metadata: {}", result);
    return std::nullopt;
  }
  if (!result.value().has_value()) {
    logger().log(fdf::INFO, "No gpio metadata provided");
  }
  return std::move(result.value());
}

void GpioRootDevice::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  uint32_t controller_id = 0;

  std::optional<fuchsia_hardware_pinimpl::Metadata> metadata = GetMetadata();

  if (metadata.has_value() && metadata->controller_id().has_value()) {
    controller_id = metadata->controller_id().value();
  } else {
    logger().log(fdf::INFO, "No controller ID provided. Assuming controller ID = 0");
  }

  zx::result scheduler_role_name_result =
      fdf_metadata::GetMetadataIfExists<fuchsia_scheduler::RoleName>(incoming());
  if (scheduler_role_name_result.is_error()) {
    logger().log(fdf::ERROR, "Failed to get scheduler role name: {}", scheduler_role_name_result);
    completer(scheduler_role_name_result.take_error());
    return;
  }
  if (scheduler_role_name_result.value().has_value()) {
    const auto& scheduler_role_name = scheduler_role_name_result.value().value();
    zx::result result = fdf::SynchronizedDispatcher::Create(
        {}, "GPIO", [](fdf_dispatcher_t*) {}, scheduler_role_name.role());
    if (result.is_error()) {
      logger().log(fdf::TRACE, "Failed to create SynchronizedDispatcher: {}", result);
      completer(result.take_error());
      return;
    }

    // If scheduler role metadata was provided, create a new dispatcher using the role, and use
    // that dispatcher instead of the default dispatcher passed to this method.
    fidl_dispatcher_.emplace(*std::move(result));

    logger().log(fdf::DEBUG, "Using dispatcher with role \"{}\"", scheduler_role_name.role());
  }

  {
    zx::result pinimpl_fidl_client =
        incoming()->Connect<fuchsia_hardware_pinimpl::Service::Device>();
    if (pinimpl_fidl_client.is_error()) {
      logger().log(fdf::TRACE, "Failed to get pinimpl protocol");
      completer(pinimpl_fidl_client.take_error());
      return;
    }

    pinimpl_.Bind(
        *std::move(pinimpl_fidl_client), fidl_dispatcher()->get(),
        fidl::ObserveTeardown(fit::bind_member<&GpioRootDevice::ClientTeardownHandler>(this)));

    if (metadata.has_value() && metadata->init_steps().has_value() &&
        !metadata->init_steps()->empty()) {
      // Process init metadata while we are still the exclusive owner of the GPIO client.
      logger().log(fdf::INFO, "Processing {} init steps", metadata->init_steps()->size());
      init_device_ = GpioInitDevice::Create(metadata->init_steps().value(), node().borrow(),
                                            logger(), controller_id, pinimpl_);
      if (!init_device_) {
        logger().log(fdf::ERROR, "Failed to create GpioInitDevice");
      } else {
        logger().log(fdf::INFO, "GpioInitDevice created successfully");
      }
    } else {
      logger().log(fdf::INFO, "No init steps provided");
    }
  }

  zx::result<fdf::OwnedChildNode> node = AddOwnedChild("gpio");
  if (node.is_error()) {
    logger().log(fdf::TRACE, "Failed to add GPIO root node: {}", node);
    completer(node.take_error());
    return;
  }
  node_ = *std::move(node);

  bool has_pins =
      metadata.has_value() && metadata->pins().has_value() && !metadata->pins()->empty();
  bool has_states = metadata.has_value() && metadata->device_pin_states().has_value() &&
                    !metadata->device_pin_states()->empty();

  if (!has_pins && !has_states) {
    logger().log(fdf::INFO, "No gpio pins or pin-states provided");
    completer(zx::ok());
    return;
  }

  std::vector<fuchsia_hardware_pinimpl::Pin> pins;
  if (has_pins) {
    pins = std::move(metadata->pins().value());
    for (size_t i = 0; i < pins.size(); ++i) {
      const auto& pin = pins[i];
      if (!pin.name().has_value()) {
        logger().log(fdf::TRACE, "Pin {} missing name", i);
        completer(zx::error(ZX_ERR_INTERNAL));
        return;
      }
      if (!pin.pin().has_value()) {
        logger().log(fdf::TRACE, "Pin {} missing pin", i);
        completer(zx::error(ZX_ERR_INTERNAL));
        return;
      }
    }
    // Make sure that the list of GPIO pins has no duplicates.
    auto gpio_cmp_lt = [](fuchsia_hardware_pinimpl::Pin& lhs, fuchsia_hardware_pinimpl::Pin& rhs) {
      return lhs.pin() < rhs.pin();
    };
    auto gpio_cmp_eq = [](fuchsia_hardware_pinimpl::Pin& lhs, fuchsia_hardware_pinimpl::Pin& rhs) {
      return lhs.pin() == rhs.pin();
    };
    std::sort(pins.begin(), pins.end(), gpio_cmp_lt);
    auto result = std::adjacent_find(pins.begin(), pins.end(), gpio_cmp_eq);
    if (result != pins.end()) {
      logger().log(fdf::TRACE, "gpio pin '{}' was published more than once", result->pin().value());
      completer(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
  }

  std::vector<fuchsia_hardware_pinimpl::DevicePinStates> device_pin_states;
  if (has_states) {
    device_pin_states = std::move(metadata->device_pin_states().value());
    for (size_t i = 0; i < device_pin_states.size(); ++i) {
      const auto& dev_state = device_pin_states[i];
      if (dev_state.name().empty()) {
        logger().log(fdf::TRACE, "DevicePinStates {} missing name", i);
        completer(zx::error(ZX_ERR_INVALID_ARGS));
        return;
      }
      for (size_t j = 0; j < dev_state.states().size(); ++j) {
        const auto& state = dev_state.states()[j];
        if (state.name().empty()) {
          logger().log(fdf::TRACE, "DevicePinStates {} state {} missing name", i, j);
          completer(zx::error(ZX_ERR_INVALID_ARGS));
          return;
        }
      }
    }
  }

  async::PostTask(
      fidl_dispatcher()->async_dispatcher(),
      [=, this, pins = std::move(pins), device_pin_states = std::move(device_pin_states),
       config = context.take_config<gpio_config::Config>(),
       completer = std::move(completer)]() mutable {
        CreatePinDevices(controller_id, pins, std::move(device_pin_states), config,
                         std::move(completer));
      });
}

void GpioRootDevice::Stop(fdf::StopCompleter completer) {
  ZX_DEBUG_ASSERT(!stop_completer_);
  stop_completer_.emplace(std::move(completer));
  pinimpl_.AsyncTeardown();
}

void GpioRootDevice::CreatePinDevices(
    const uint32_t controller_id, std::span<fuchsia_hardware_pinimpl::Pin> pins,
    std::vector<fuchsia_hardware_pinimpl::DevicePinStates> device_pin_states,
    gpio_config::Config config, fdf::StartCompleter completer) {
  for (const auto& pin : pins) {
    fbl::AllocChecker ac;
    children_.emplace_back(new (&ac) GpioDevice(pinimpl_.Clone(), pin.pin().value(),
                                                pin.name().value(), pin.id(), logger()));
    if (!ac.check()) {
      logger().log(fdf::ERROR, "Failed to allocate memory for pin");
      completer(zx::error(ZX_ERR_NO_MEMORY));
      return;
    }
  }

  for (auto& device_states : device_pin_states) {
    fbl::AllocChecker ac;
    pin_states_children_.emplace_back(new (&ac) PinStatesDevice(
        pinimpl_.Clone(), std::move(device_states), controller_id, logger()));
    if (!ac.check()) {
      logger().log(fdf::ERROR, "Failed to allocate memory for pin-states");
      completer(zx::error(ZX_ERR_NO_MEMORY));
      return;
    }
  }

  async::PostTask(dispatcher(),
                  [this, config = config, completer = std::move(completer)]() mutable {
                    ServePinDevices(config, std::move(completer));
                  });
}

void GpioRootDevice::ServePinDevices(gpio_config::Config config, fdf::StartCompleter completer) {
  for (std::unique_ptr<GpioDevice>& child : children_) {
    zx::result<> result = child->AddServices(incoming(), outgoing(), config);
    if (result.is_error()) {
      logger().log(fdf::ERROR, "Failed to serve pin devices: {}", result);
      completer(result.take_error());
      return;
    }
  }

  for (std::unique_ptr<PinStatesDevice>& child : pin_states_children_) {
    zx::result<> result = child->AddServices(incoming(), outgoing());
    if (result.is_error()) {
      logger().log(fdf::ERROR, "Failed to serve pin-states devices: {}", result);
      completer(result.take_error());
      return;
    }
  }

  async::PostTask(fidl_dispatcher()->async_dispatcher(),
                  [this, config = config, completer = std::move(completer)]() mutable {
                    AddPinDevices(config, std::move(completer));
                  });
}

void GpioRootDevice::AddPinDevices(gpio_config::Config config, fdf::StartCompleter completer) {
  for (std::unique_ptr<GpioDevice>& child : children_) {
    if (zx::result<> result = child->AddDevice(node_.node_.borrow(), logger(), config);
        result.is_error()) {
      logger().log(fdf::ERROR, "Failed to add pin device: {}", result);
      completer(result.take_error());
      return;
    }
  }

  for (std::unique_ptr<PinStatesDevice>& child : pin_states_children_) {
    if (zx::result<> result = child->AddDevice(node_.node_.borrow()); result.is_error()) {
      logger().log(fdf::ERROR, "Failed to add pin-states device: {}", result);
      completer(result.take_error());
      return;
    }

    if (zx::result<> result = child->ApplyDefaultState(); result.is_error()) {
      logger().log(fdf::ERROR, "Failed to apply default state: {}", result);
      completer(result.take_error());
      return;
    }
  }

  logger().log(fdf::INFO, "All pin/pin-states devices added successfully");
  completer(zx::ok());
}

void GpioRootDevice::ClientTeardownHandler() {
  async::PostTask(dispatcher(), [this]() {
    if (stop_completer_) {
      (*stop_completer_)(zx::ok());
    }
  });
}

std::unique_ptr<GpioInitDevice> GpioInitDevice::Create(
    std::span<fuchsia_hardware_pinimpl::InitStep> init_steps,
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> node, fdf::Logger& logger,
    uint32_t controller_id, fdf::WireSharedClient<fuchsia_hardware_pinimpl::PinImpl>& pinimpl) {
  std::unique_ptr device = std::make_unique<GpioInitDevice>();
  zx_status_t status = ConfigureGpios(init_steps, pinimpl, logger);
  if (status != ZX_OK) {
    logger.log(fdf::ERROR, "ConfigureGpios failed: {}", zx_status_get_string(status));
    // Return without adding the init device if some GPIOs could not be configured. This will
    // prevent all drivers that depend on the initial state from binding, which should make it
    // more obvious that something has gone wrong.
    return {};
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> props{
      fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
      fdf::MakeProperty2(bind_fuchsia::ID, controller_id),
  };

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> result =
      fdf::AddChild(node, logger, "gpio-init", props, {});
  if (result.is_error()) {
    logger.log(fdf::ERROR, "Failed to add gpio-init node: {}", result);
    return {};
  }

  device->controller_.Bind(*std::move(result));
  return device;
}

zx_status_t GpioInitDevice::ConfigureGpios(
    std::span<fuchsia_hardware_pinimpl::InitStep> init_steps,
    fdf::WireSharedClient<fuchsia_hardware_pinimpl::PinImpl>& pinimpl, fdf::Logger& logger) {
  // Stop processing the list if any call returns an error so that GPIOs are not accidentally put
  // into an unexpected state.
  for (const auto& step : init_steps) {
    fdf::Arena arena('GPIO');

    if (step.Which() == fuchsia_hardware_pinimpl::InitStep::Tag::kDelay) {
      zx::nanosleep(zx::deadline_after(zx::duration(step.delay().value())));
      continue;
    }
    if (step.Which() != fuchsia_hardware_pinimpl::InitStep::Tag::kCall) {
      logger.log(fdf::ERROR, "Invalid GPIO init metadata");
      return ZX_ERR_INVALID_ARGS;
    }

    const auto& call = step.call()->call();
    const uint32_t pin = step.call()->pin();
    if (call.Which() == fuchsia_hardware_pinimpl::InitCall::Tag::kPinConfig) {
      const auto& config = call.pin_config().value();
      auto result = pinimpl.sync().buffer(arena)->Configure(pin, fidl::ToWire(arena, config));
      if (!result.ok()) {
        logger.log(fdf::ERROR, "Call to Configure failed for pin {}: {}", pin,
                   result.status_string());
        return result.status();
      }
      if (result->is_error()) {
        logger.log(fdf::ERROR, "Configure failed for pin {}: {}", pin,
                   zx_status_get_string(result->error_value()));
        return result->error_value();
      }

      const auto& driver_strength_ua = config.drive_strength_ua();
      if (driver_strength_ua.has_value()) {
        if (!result->value()->new_config.has_drive_strength_ua()) {
          logger.log(fdf::WARN, "Drive strength not returned for {}", pin);
          return ZX_ERR_BAD_STATE;
        }
        if (result->value()->new_config.drive_strength_ua() != driver_strength_ua.value()) {
          logger.log(fdf::WARN, "Actual drive strength ({}) doesn't match expected ({}) for {}",
                     result->value()->new_config.drive_strength_ua(), driver_strength_ua.value(),
                     pin);
          return ZX_ERR_BAD_STATE;
        }
      }

      const auto& drive_type = config.drive_type();
      if (drive_type.has_value()) {
        if (!result->value()->new_config.has_drive_type()) {
          logger.log(fdf::WARN, "Drive type not returned for {}", pin);
          return ZX_ERR_BAD_STATE;
        }
        if (result->value()->new_config.drive_type() != drive_type.value()) {
          logger.log(fdf::WARN, "Actual drive type ({}) doesn't match expected ({}) for {}",
                     static_cast<uint32_t>(result->value()->new_config.drive_type()),
                     static_cast<uint32_t>(drive_type.value()), pin);
          return ZX_ERR_BAD_STATE;
        }
      }

      const auto& power_source = config.power_source();
      if (power_source.has_value()) {
        if (!result->value()->new_config.has_power_source()) {
          logger.log(fdf::WARN, "Power source not returned for {}", pin);
          return ZX_ERR_BAD_STATE;
        }
        if (result->value()->new_config.power_source() != power_source.value()) {
          logger.log(fdf::WARN, "Actual power source ({}) doesn't match expected ({}) for {}",
                     result->value()->new_config.power_source(), power_source.value(), pin);
          return ZX_ERR_BAD_STATE;
        }
      }
    } else if (call.Which() == fuchsia_hardware_pinimpl::InitCall::Tag::kBufferMode) {
      auto result = pinimpl.sync().buffer(arena)->SetBufferMode(
          pin, fidl::ToWire(arena, call.buffer_mode().value()));
      if (!result.ok()) {
        logger.log(fdf::ERROR, "Call to SetBufferMode failed for pin {}: {}", pin,
                   result.status_string());
        return result.status();
      }
      if (result->is_error()) {
        logger.log(fdf::ERROR, "SetBufferMode failed for pin {}: {}", pin,
                   zx_status_get_string(result->error_value()));
        return result->error_value();
      }
    }
  }

  return ZX_OK;
}

}  // namespace gpio

FUCHSIA_DRIVER_EXPORT2(gpio::GpioRootDevice);
