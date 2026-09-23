// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_VREG_CPP_FAKE_VREG_H_
#define LIB_DRIVER_FAKE_VREG_CPP_FAKE_VREG_H_

#include <fidl/fuchsia.hardware.vreg/cpp/fidl.h>
#include <fidl/fuchsia.hardware.vreg/cpp/wire_test_base.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstdint>
#include <optional>
#include <string>

namespace fdf_fake {

class FakeVreg final : public fidl::testing::WireTestBase<fuchsia_hardware_vreg::Vreg> {
 public:
  explicit FakeVreg(
      async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher());

  void SetVoltageStep(SetVoltageStepRequestView request,
                      SetVoltageStepCompleter::Sync& completer) override;
  void GetVoltageStep(GetVoltageStepCompleter::Sync& completer) override;
  void SetState(SetStateRequestView request, SetStateCompleter::Sync& completer) override;
  void Enable(EnableCompleter::Sync& completer) override;
  void Disable(DisableCompleter::Sync& completer) override;
  void GetRegulatorParams(GetRegulatorParamsCompleter::Sync& completer) override;

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void SetRegulatorParams(uint32_t min_uv, uint32_t step_size_uv, uint32_t num_steps);

  void set_set_voltage_step_result(zx::result<> result) { set_voltage_step_result_ = result; }
  void set_get_voltage_step_result(zx::result<uint32_t> result) {
    get_voltage_step_result_ = result;
  }
  void set_set_state_result(zx::result<> result) { set_state_result_ = result; }
  void set_enable_result(zx::result<> result) { enable_result_ = result; }
  void set_disable_result(zx::result<> result) { disable_result_ = result; }
  void set_get_regulator_params_result(zx::result<> result) {
    get_regulator_params_result_ = result;
  }

  bool enabled() const { return last_enable_request_.value_or(false); }
  uint32_t voltage_step() const { return voltage_step_; }
  uint32_t voltage_uv() const {
    uint32_t result = 0;
    ZX_ASSERT(!__builtin_mul_overflow(voltage_step_, step_size_uv_, &result));
    ZX_ASSERT(!__builtin_add_overflow(min_uv_, result, &result));
    return result;
  }

  std::optional<bool> take_enable_request();
  std::optional<fuchsia_hardware_vreg::RegulatorMode> take_mode_request();

  void Bind(async_dispatcher_t* dispatcher,
            fidl::ServerEnd<fuchsia_hardware_vreg::Vreg> server_end);

  fuchsia_hardware_vreg::Service::InstanceHandler CreateInstanceHandler();

 private:
  uint32_t min_uv_ = 0;
  uint32_t step_size_uv_ = 0;
  uint32_t num_steps_ = 0;
  uint32_t voltage_step_ = 0;

  std::optional<bool> last_enable_request_;
  std::optional<fuchsia_hardware_vreg::RegulatorMode> last_mode_request_;

  zx::result<> set_voltage_step_result_ = zx::ok();
  zx::result<uint32_t> get_voltage_step_result_ = zx::ok(0);
  zx::result<> set_state_result_ = zx::ok();
  zx::result<> enable_result_ = zx::ok();
  zx::result<> disable_result_ = zx::ok();
  zx::result<> get_regulator_params_result_ = zx::ok();

  fidl::ServerBindingGroup<fuchsia_hardware_vreg::Vreg> bindings_;
  async_dispatcher_t* dispatcher_;
};

}  // namespace fdf_fake

#endif  // LIB_DRIVER_FAKE_VREG_CPP_FAKE_VREG_H_
