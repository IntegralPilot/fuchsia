// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.clock/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>

namespace fake_device {

class FakeDeviceDriver : public fdf::DriverBase2,
                         public fidl::WireServer<fuchsia_hardware_clock::Clock> {
 public:
  FakeDeviceDriver() : fdf::DriverBase2("fake-device") {}

  zx::result<> Start(fdf::DriverContext context) final {
    fdf::info("Starting fake-device driver");

    incoming_ = context.take_incoming();

    // 1. Connect to pdev
    auto pdev_result = fdf::PDev::Connect(incoming_);
    if (pdev_result.is_error()) {
      fdf::error("Failed to connect to pdev: {}", pdev_result);
      return pdev_result.take_error();
    }
    auto pdev = std::move(pdev_result.value());

    // 2. Connect to clock service "my-clock"
    auto clock_connect = incoming_->Connect<fuchsia_hardware_clock::Service::Clock>("my-clock");
    if (clock_connect.is_error()) {
      fdf::error("Failed to connect to clock service: {}", clock_connect);
      return clock_connect.take_error();
    }
    auto clock_client = fidl::SyncClient(std::move(clock_connect.value()));

    // Call clock methods
    auto prop_result = clock_client->GetProperties();
    if (prop_result.is_error()) {
      fdf::error("Failed to get clock properties: {}",
                 prop_result.error_value().FormatDescription());
      return zx::error(prop_result.error_value().status());
    }
    fdf::info("Connected to clock: name={}, id={}", prop_result.value().name(),
              prop_result.value().id());

    auto enable_result = clock_client->Enable();
    if (enable_result.is_error()) {
      fdf::error("Failed to enable clock: {}", enable_result.error_value().FormatDescription());
      if (enable_result.error_value().is_framework_error()) {
        return zx::error(enable_result.error_value().framework_error().status());
      } else {
        return zx::error(enable_result.error_value().domain_error());
      }
    }

    // 6. Export to devfs
    zx::result connector = devfs_connector_.Bind(dispatcher());
    if (connector.is_error()) {
      fdf::error("Failed to bind devfs connector: {}", connector);
      return connector.take_error();
    }

    fuchsia_driver_framework::DevfsAddArgs devfs_args{
        {.connector = std::move(connector.value()), .class_name{"test"}}};

    // We add a child node that has devfs args.
    // The node name will be used as the filename in devfs if we don't specify otherwise?
    // Actually, class_name is "test", so it goes to /dev/class/test/<node_name>.
    // Let's name the child node "fake-device".
    auto child = AddOwnedChild("fake-device", devfs_args);
    if (child.is_error()) {
      fdf::error("Failed to add devfs child: {}", child);
      return child.take_error();
    }
    child_node_ = std::move(child.value());

    fdf::info("fake-device driver started successfully!");

    return zx::ok();
  }

  // fidl::WireServer<fuchsia_hardware_clock::Clock> implementation (dummy)
  void Enable(EnableCompleter::Sync& completer) override { completer.ReplySuccess(); }
  void Disable(DisableCompleter::Sync& completer) override { completer.ReplySuccess(); }
  void IsEnabled(IsEnabledCompleter::Sync& completer) override { completer.ReplySuccess(true); }
  void SetRate(SetRateRequestView request, SetRateCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void QuerySupportedRate(QuerySupportedRateRequestView request,
                          QuerySupportedRateCompleter::Sync& completer) override {
    completer.ReplySuccess(request->hz_in);
  }
  void GetRate(GetRateCompleter::Sync& completer) override { completer.ReplySuccess(0); }
  void SetInput(SetInputRequestView request, SetInputCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void GetNumInputs(GetNumInputsCompleter::Sync& completer) override { completer.ReplySuccess(0); }
  void GetInput(GetInputCompleter::Sync& completer) override { completer.ReplySuccess(0); }
  void GetProperties(GetPropertiesCompleter::Sync& completer) override {
    completer.Reply(0, fidl::StringView::FromExternal("fake-device"));
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_clock::Clock> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method ordinal {}", metadata.method_ordinal);
  }

 private:
  void Connect(fidl::ServerEnd<fuchsia_hardware_clock::Clock> request) {
    bindings_.AddBinding(dispatcher(), std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  fdf::OwnedChildNode child_node_;
  driver_devfs::Connector<fuchsia_hardware_clock::Clock> devfs_connector_{
      fit::bind_member<&FakeDeviceDriver::Connect>(this)};
  fidl::ServerBindingGroup<fuchsia_hardware_clock::Clock> bindings_;
  std::shared_ptr<fdf::Namespace> incoming_;
};

}  // namespace fake_device

FUCHSIA_DRIVER_EXPORT2(fake_device::FakeDeviceDriver);
