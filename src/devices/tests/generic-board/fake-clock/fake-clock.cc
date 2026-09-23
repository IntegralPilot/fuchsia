// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.clock/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fidl/cpp/wire/channel.h>

namespace fake_clock {

class FakeClockDevice : public fidl::Server<fuchsia_hardware_clock::Clock> {
 public:
  FakeClockDevice(uint32_t id, std::string name) : id_(id), name_(std::move(name)) {}

  void Enable(EnableCompleter::Sync& completer) override {
    fdf::info("Clock {} (id {}) Enabled", name_, id_);
    completer.Reply(zx::ok());
  }

  void Disable(DisableCompleter::Sync& completer) override {
    fdf::info("Clock {} (id {}) Disabled", name_, id_);
    completer.Reply(zx::ok());
  }

  void IsEnabled(IsEnabledCompleter::Sync& completer) override { completer.Reply(zx::ok(true)); }

  void SetRate(SetRateRequest& request, SetRateCompleter::Sync& completer) override {
    fdf::info("Clock {} (id {}) rate set to {} Hz", name_, id_, request.hz());
    completer.Reply(zx::ok());
  }

  void QuerySupportedRate(QuerySupportedRateRequest& request,
                          QuerySupportedRateCompleter::Sync& completer) override {
    completer.Reply(zx::ok(request.hz_in()));
  }

  void GetRate(GetRateCompleter::Sync& completer) override {
    completer.Reply(zx::ok(1000000000));  // 1 GHz
  }

  void SetInput(SetInputRequest& request, SetInputCompleter::Sync& completer) override {
    completer.Reply(zx::ok());
  }

  void GetNumInputs(GetNumInputsCompleter::Sync& completer) override { completer.Reply(zx::ok(0)); }

  void GetInput(GetInputCompleter::Sync& completer) override { completer.Reply(zx::ok(0)); }

  void GetProperties(GetPropertiesCompleter::Sync& completer) override {
    completer.Reply({{.id = id_, .name = name_}});
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_clock::Clock> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method ordinal {}", metadata.method_ordinal);
  }

 private:
  uint32_t id_;
  std::string name_;
};

class FakeClockDriver : public fdf::DriverBase2 {
 public:
  FakeClockDriver() : fdf::DriverBase2("fake-clock") {}

  zx::result<> Start(fdf::DriverContext context) final {
    fdf::info("Starting fake-clock driver");

    incoming_ = context.take_incoming();
    auto pdev_result = fdf::PDev::Connect(incoming_, "pdev");
    if (pdev_result.is_error()) {
      fdf::error("Failed to connect to pdev: {}", pdev_result);
      return pdev_result.take_error();
    }
    auto pdev = std::move(pdev_result.value());

    // Hardcoded clock config: name="my-clock", id=1
    std::string clock_name = "my-clock";
    uint32_t clock_id = 1;

    auto clock_device = std::make_unique<FakeClockDevice>(clock_id, clock_name);

    // Expose service
    fuchsia_hardware_clock::Service::InstanceHandler instance_handler{
        {.clock = bindings_.CreateHandler(clock_device.get(),
                                          fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure)}};

    auto result = outgoing()->AddService<fuchsia_hardware_clock::Service>(
        std::move(instance_handler), clock_name);
    if (result.is_error()) {
      fdf::error("Failed to add service {}: {}", clock_name, result);
      return result.take_error();
    }

    std::vector<fuchsia_driver_framework::NodeProperty2> props{
        fdf::MakeProperty2("fuchsia.NAME", clock_name),
        fdf::MakeProperty2("fuchsia.ID", clock_id),
    };

    auto offers = std::vector{fdf::MakeOffer2<fuchsia_hardware_clock::Service>(clock_name)};

    auto child = AddChild(clock_name, props, offers);
    if (child.is_error()) {
      fdf::error("Failed to add child {}: {}", clock_name, child);
      return child.take_error();
    }

    clock_devices_.push_back(std::move(clock_device));

    return zx::ok();
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_clock::Clock> bindings_;
  std::vector<std::unique_ptr<FakeClockDevice>> clock_devices_;
  std::shared_ptr<fdf::Namespace> incoming_;
};

}  // namespace fake_clock

FUCHSIA_DRIVER_EXPORT2(fake_clock::FakeClockDriver);
