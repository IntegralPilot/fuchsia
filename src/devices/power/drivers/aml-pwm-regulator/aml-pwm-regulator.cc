// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/aml-pwm-regulator/aml-pwm-regulator.h"

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <string>

#include <bind/fuchsia/cpp/bind.h>

namespace aml_pwm_regulator {

AmlPwmRegulator::AmlPwmRegulator(const VregMetadata& metadata,
                                 fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> pwm_proto_client)
    : name_(metadata.name().value()),
      min_voltage_uv_(metadata.min_voltage_uv().value()),
      voltage_step_uv_(metadata.voltage_step_uv().value()),
      num_steps_(metadata.num_steps().value()),
      current_step_(metadata.num_steps().value()),
      pwm_proto_client_(std::move(pwm_proto_client)) {}

void AmlPwmRegulator::SetVoltageStep(SetVoltageStepRequestView request,
                                     SetVoltageStepCompleter::Sync& completer) {
  if (request->step >= num_steps_) {
    fdf::error("Requested step ({}) is larger than allowed (total number of steps {}).",
               request->step, num_steps_);
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (request->step == current_step_) {
    completer.ReplySuccess();
    return;
  }

  auto config_result = pwm_proto_client_->GetConfig();
  if (!config_result.ok() || config_result->is_error()) {
    auto status = config_result.ok() ? config_result->error_value() : config_result.status();
    fdf::error("Unable to get PWM config. {}", zx_status_get_string(status));
    completer.ReplyError(status);
    return;
  }

  if (config_result->value()->config.period_ns == 0) {
    fdf::error("PWM period config of 0ns is invalid.");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg = {
      .polarity = config_result->value()->config.polarity,
      .period_ns = config_result->value()->config.period_ns,
      .duty_cycle =
          static_cast<float>((num_steps_ - 1 - request->step) * 100.0 / ((num_steps_ - 1) * 1.0)),
      .mode_config =
          fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on)),
  };

  auto result = pwm_proto_client_->SetConfig(cfg);
  if (!result.ok()) {
    fdf::error("Unable to configure PWM. {}", result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Unable to configure PWM. {}", zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }
  current_step_ = request->step;

  completer.ReplySuccess();
}

void AmlPwmRegulator::SetState(SetStateRequestView request, SetStateCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void AmlPwmRegulator::GetVoltageStep(GetVoltageStepCompleter::Sync& completer) {
  completer.ReplySuccess(current_step_);
}

void AmlPwmRegulator::GetRegulatorParams(GetRegulatorParamsCompleter::Sync& completer) {
  completer.ReplySuccess(min_voltage_uv_, voltage_step_uv_, num_steps_);
}

void AmlPwmRegulator::Enable(EnableCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void AmlPwmRegulator::Disable(DisableCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx::result<std::unique_ptr<AmlPwmRegulator>> AmlPwmRegulator::Create(const VregMetadata& metadata,
                                                                     AmlPwmRegulatorDriver& driver,
                                                                     fdf::Namespace& incoming) {
  auto connect_result = incoming.Connect<fuchsia_hardware_pwm::Service::Pwm>("pwm");
  if (connect_result.is_error()) {
    fdf::error("Unable to connect to fidl protocol - status: {}", connect_result.status_string());
    return connect_result.take_error();
  }

  const auto& name = metadata.name().value();
  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> pwm_proto_client(
      std::move(connect_result.value()));
  auto result = pwm_proto_client->Enable();
  if (!result.ok()) {
    fdf::error("VREG({}): Unable to enable PWM - {}", name.c_str(), result.status_string());
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (result->is_error()) {
    fdf::error("VREG({}): Unable to enable PWM - {}", name.c_str(),
               zx_status_get_string(result->error_value()));
    return result->take_error();
  }
  auto device = std::make_unique<AmlPwmRegulator>(metadata, std::move(pwm_proto_client));

  {
    auto result = driver.outgoing()->AddService<fuchsia_hardware_vreg::Service>(
        fuchsia_hardware_vreg::Service::InstanceHandler({
            .vreg = device->bindings_.CreateHandler(
                device.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                fidl::kIgnoreBindingClosure),
        }),
        name);
    if (result.is_error()) {
      fdf::error("Failed to add Device service {}", result);
      return result.take_error();
    }
  }

  std::vector offers = {fdf::MakeOffer2<fuchsia_hardware_vreg::Service>(name)};

  std::vector properties = {
      fdf::MakeProperty2(bind_fuchsia::NAME, name),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.vreg.Service"),
  };

  zx::result child = fdf::AddChild(driver.node(), driver.logger(), name, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  device->controller_.Bind(std::move(child.value()));

  return zx::ok(std::move(device));
}

zx::result<> AmlPwmRegulatorDriver::Start(fdf::DriverContext context) {
  zx::result pdev_client =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client.value())};
  zx::result metadata_result = pdev.GetFidlMetadata<fuchsia_hardware_vreg::VregMetadata>();
  if (metadata_result.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata_result);
    return metadata_result.take_error();
  }
  const auto& metadata = metadata_result.value();

  // Validate
  if (!metadata.name().has_value()) {
    fdf::error("Metadata missing name field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.min_voltage_uv().has_value()) {
    fdf::error("Metadata missing min_voltage_uv field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.voltage_step_uv().has_value()) {
    fdf::error("Metadata missing voltage_step_uv field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.num_steps().has_value()) {
    fdf::error("Metadata missing num_steps field");
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Build Voltage Regulator
  auto regulator = AmlPwmRegulator::Create(metadata, *this, context.incoming());
  if (regulator.is_error()) {
    return regulator.take_error();
  }
  regulators_ = std::move(*regulator);
  return zx::ok();
}

}  // namespace aml_pwm_regulator

FUCHSIA_DRIVER_EXPORT2(aml_pwm_regulator::AmlPwmRegulatorDriver);
