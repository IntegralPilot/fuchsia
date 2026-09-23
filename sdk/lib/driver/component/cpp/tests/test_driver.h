// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_TESTS_TEST_DRIVER_H_
#define LIB_DRIVER_COMPONENT_CPP_TESTS_TEST_DRIVER_H_

#include <fidl/fuchsia.driver.component.test/cpp/driver/wire.h>
#include <fidl/fuchsia.driver.component.test/cpp/wire.h>
#include <fidl/fuchsia.driver.framework/cpp/wire_messaging.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/symbols/symbols.h>

extern bool g_driver_stopped;

class TestDriver : public fdf::DriverBase2,
                   public fidl::WireServer<fuchsia_driver_component_test::ZirconProtocol>,
                   public fdf::WireServer<fuchsia_driver_component_test::DriverProtocol>,
                   public fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController> {
 public:
  TestDriver()
      : fdf::DriverBase2("test_driver"),
        devfs_connector_(fit::bind_member<&TestDriver::Connect>(this)) {}

  ~TestDriver() override;

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;

  void Stop(fdf::StopCompleter completer) override;

  zx::result<> InitSyncCompat();
  void BeginInitAsyncCompat(fit::callback<void(zx::result<>)> completed);

  zx::result<> ExportDevfsNodeSync();
  zx::result<> ServeDriverService();
  zx::result<> ServeZirconService();

  zx::result<> ValidateIncomingDriverService(
      std::string_view instance = component::kDefaultInstance);
  zx::result<> ValidateIncomingZirconService(
      std::string_view instance = component::kDefaultInstance);

  void CreateChildNodeSync();

  void CreateChildNodeAsync();

  bool async_added_child() const { return async_added_child_; }
  bool sync_added_child() const { return sync_added_child_; }

  // fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController>
  void on_fidl_error(fidl::UnbindInfo error) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override;

 private:
  // fidl::WireServer<fuchsia_driver_component_test::ZirconProtocol>
  void ZirconMethod(ZirconMethodCompleter::Sync& completer) override { completer.ReplySuccess(); }

  // fdf::WireServer<fuchsia_driver_component_test::DriverProtocol>
  void DriverMethod(fdf::Arena& arena, DriverMethodCompleter::Sync& completer) override {
    fdf::Arena reply_arena('DRVR');
    completer.buffer(reply_arena).ReplySuccess();
  }

  // driver_devfs::Connector<fuchsia_driver_component_test::ZirconProtocol>
  void Connect(fidl::ServerEnd<fuchsia_driver_component_test::ZirconProtocol> request) {
    zircon_bindings_.AddBinding(dispatcher(), std::move(request), this,
                                fidl::kIgnoreBindingClosure);
  }

  fuchsia_driver_component_test::ZirconService::InstanceHandler GetInstanceHandlerZircon() {
    return fuchsia_driver_component_test::ZirconService::InstanceHandler({
        .device = zircon_bindings_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
    });
  }

  fuchsia_driver_component_test::DriverService::InstanceHandler GetInstanceHandlerDriver() {
    return fuchsia_driver_component_test::DriverService::InstanceHandler({
        .device = driver_bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                                 fidl::kIgnoreBindingClosure),
    });
  }

  compat::SyncInitializedDeviceServer sync_device_server_;
  compat::AsyncInitializedDeviceServer async_device_server_;

  fidl::WireClient<fuchsia_driver_framework::Node> node_client_;
  bool async_added_child_ = false;
  bool sync_added_child_ = false;

  fidl::ServerBindingGroup<fuchsia_driver_component_test::ZirconProtocol> zircon_bindings_;
  driver_devfs::Connector<fuchsia_driver_component_test::ZirconProtocol> devfs_connector_;

  fdf::ServerBindingGroup<fuchsia_driver_component_test::DriverProtocol> driver_bindings_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> devfs_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> devfs_node_controller_;

  std::optional<fdf::StopCompleter> stop_completer_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> child_controller_;

  std::optional<fdf::SynchronizedDispatcher> not_shutdown_manually_dispatcher_;
  std::shared_ptr<fdf::Namespace> incoming_;
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
  std::string node_name_;
};

class StartFailTestDriver : public TestDriver {
 public:
  using TestDriver::TestDriver;
  static DriverRegistration GetDriverRegistration();
  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
};

#endif  // LIB_DRIVER_COMPONENT_CPP_TESTS_TEST_DRIVER_H_
