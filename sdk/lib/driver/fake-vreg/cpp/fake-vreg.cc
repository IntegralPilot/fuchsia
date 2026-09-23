// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-vreg/cpp/fake-vreg.h>

namespace fdf_fake {

FakeVreg::FakeVreg(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

void FakeVreg::SetVoltageStep(SetVoltageStepRequestView request,
                              SetVoltageStepCompleter::Sync& completer) {
  if (set_voltage_step_result_.is_error()) {
    completer.ReplyError(set_voltage_step_result_.status_value());
    return;
  }
  if (request->step >= num_steps_) {
    // Optionally handle invalid step
  }
  voltage_step_ = request->step;
  completer.ReplySuccess();
}

void FakeVreg::GetVoltageStep(GetVoltageStepCompleter::Sync& completer) {
  if (get_voltage_step_result_.is_error()) {
    completer.ReplyError(get_voltage_step_result_.status_value());
    return;
  }
  completer.ReplySuccess(voltage_step_);
}

void FakeVreg::SetState(SetStateRequestView request, SetStateCompleter::Sync& completer) {
  if (set_state_result_.is_error()) {
    completer.ReplyError(set_state_result_.status_value());
    return;
  }
  if (request->has_step()) {
    voltage_step_ = request->step();
  }
  if (request->has_enable()) {
    last_enable_request_ = request->enable();
  }
  if (request->has_mode()) {
    last_mode_request_ = request->mode();
  }
  completer.ReplySuccess();
}

void FakeVreg::Enable(EnableCompleter::Sync& completer) {
  if (enable_result_.is_error()) {
    completer.ReplyError(enable_result_.status_value());
    return;
  }
  last_enable_request_ = true;
  completer.ReplySuccess();
}

void FakeVreg::Disable(DisableCompleter::Sync& completer) {
  if (disable_result_.is_error()) {
    completer.ReplyError(disable_result_.status_value());
    return;
  }
  last_enable_request_ = false;
  completer.ReplySuccess();
}

void FakeVreg::GetRegulatorParams(GetRegulatorParamsCompleter::Sync& completer) {
  if (get_regulator_params_result_.is_error()) {
    completer.ReplyError(get_regulator_params_result_.status_value());
    return;
  }
  completer.ReplySuccess(min_uv_, step_size_uv_, num_steps_);
}

void FakeVreg::SetRegulatorParams(uint32_t min_uv, uint32_t step_size_uv, uint32_t num_steps) {
  min_uv_ = min_uv;
  step_size_uv_ = step_size_uv;
  num_steps_ = num_steps;
}

std::optional<bool> FakeVreg::take_enable_request() {
  std::optional<bool> result = last_enable_request_;
  last_enable_request_.reset();
  return result;
}

std::optional<fuchsia_hardware_vreg::RegulatorMode> FakeVreg::take_mode_request() {
  std::optional<fuchsia_hardware_vreg::RegulatorMode> result = last_mode_request_;
  last_mode_request_.reset();
  return result;
}

void FakeVreg::Bind(async_dispatcher_t* dispatcher,
                    fidl::ServerEnd<fuchsia_hardware_vreg::Vreg> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

fuchsia_hardware_vreg::Service::InstanceHandler FakeVreg::CreateInstanceHandler() {
  return fuchsia_hardware_vreg::Service::InstanceHandler({
      .vreg = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
  });
}

}  // namespace fdf_fake
