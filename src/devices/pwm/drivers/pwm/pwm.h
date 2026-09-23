// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_PWM_DRIVERS_PWM_PWM_H_
#define SRC_DEVICES_PWM_DRIVERS_PWM_PWM_H_

#include <fidl/fuchsia.hardware.pwm/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pwm/cpp/wire.h>
#include <fidl/fuchsia.hardware.pwmimpl/cpp/driver/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include <mutex>
#include <optional>
#include <string>
namespace pwm {

class PwmChannel : public fidl::WireServer<fuchsia_hardware_pwm::Pwm> {
 public:
  static constexpr std::string_view kClassName = "pwm";

  explicit PwmChannel(uint32_t id, std::optional<uint32_t> global_id,
                      std::optional<std::string> name, async_dispatcher_t* dispatcher,
                      fdf::ClientEnd<fuchsia_hardware_pwmimpl::PwmImpl> pwm_impl)
      : id_(id),
        global_id_(global_id),
        name_(std::move(name)),
        pwm_impl_(std::move(pwm_impl)),
        dispatcher_(dispatcher) {}

  zx::result<> Init(std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent);

  // fidl::WireServer<fuchsia_hardware_pwm::Pwm> implementation.
  void GetConfig(GetConfigCompleter::Sync& completer) override;
  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override;
  void Enable(EnableCompleter::Sync& completer) override;
  void Disable(DisableCompleter::Sync& completer) override;

 private:
  void Connect(fidl::ServerEnd<fuchsia_hardware_pwm::Pwm> request);

  // ID of the pwm channel.
  const uint32_t id_;
  const std::optional<uint32_t> global_id_;
  const std::optional<std::string> name_;

  fdf::WireSyncClient<fuchsia_hardware_pwmimpl::PwmImpl> pwm_impl_;

  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_hardware_pwm::Pwm> bindings_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  driver_devfs::Connector<fuchsia_hardware_pwm::Pwm> devfs_connector_{
      fit::bind_member<&PwmChannel::Connect>(this)};
};

class Pwm : public fdf::DriverBase2 {
 public:
  static constexpr std::string_view kDriverName = "pwm";

  explicit Pwm() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

 private:
  std::vector<std::unique_ptr<PwmChannel>> pwm_channels_;
};

}  // namespace pwm

#endif  // SRC_DEVICES_PWM_DRIVERS_PWM_PWM_H_
