// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h"

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <lib/async-loop/loop.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/watcher.h>
#include <zircon/status.h>

#include <iostream>

#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <usb/usb.h>

namespace {

class EventWatcher : public fidl::WireServer<fuchsia_hardware_usb_peripheral::Events> {
 public:
  EventWatcher(async::Loop& loop, fidl::ServerEnd<fuchsia_hardware_usb_peripheral::Events> svc,
               size_t functions)
      : loop_(loop), functions_(functions) {
    fidl::BindServer(loop.dispatcher(), std::move(svc), this);
  }

  void FunctionRegistered(FunctionRegisteredCompleter::Sync& completer) override {
    functions_registered_++;
    if (all_functions_registered()) {
      loop_.Quit();
      completer.Close(ZX_ERR_CANCELED);
    } else {
      completer.Reply();
    }
  }

  void FunctionsCleared(FunctionsClearedCompleter::Sync& completer) override {
    all_functions_cleared_ = true;
    loop_.Quit();
    completer.Close(ZX_ERR_CANCELED);
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_peripheral::Events> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  bool all_functions_registered() const { return functions_registered_ == functions_; }
  bool all_functions_cleared() const { return all_functions_cleared_; }

 private:
  async::Loop& loop_;
  const size_t functions_;
  size_t functions_registered_ = 0;
  bool all_functions_cleared_ = false;
};

}  // namespace

namespace usb_virtual {

zx::result<BusLauncher> BusLauncher::Create(
    std::vector<fuchsia_component_test::Capability> exposes) {
  auto loop = std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);
  loop->StartThread();

  auto realm_args = fuchsia_driver_test::RealmArgs();
  realm_args.root_driver("fuchsia-boot:///dtr#meta/usb-virtual-bus.cm");
  exposes.emplace_back(fuchsia_component_test::Capability::WithService(
      fuchsia_component_test::Service{{.name = fuchsia_hardware_usb_peripheral::Service::Name}}));

  auto realm_builder = component_testing::RealmBuilder::Create();
  driver_test_realm::Setup(realm_builder, loop->dispatcher(),
                           driver_test_realm::OptionsBuilder().driver_exposes(exposes).Build(),
                           std::move(realm_args));
  auto realm = realm_builder.Build(loop->dispatcher());

  zx::result<> boot_result = driver_test_realm::WaitForBootup(realm);
  if (boot_result.is_error()) {
    return boot_result.take_error();
  }

  // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
  // Open dev.
  auto [dev_client, dev_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  zx_status_t status = realm.component().exposed()->Open(
      "dev-topological", fuchsia::io::PERM_READABLE, {}, dev_server.TakeChannel());
  if (status != ZX_OK) {
    std::cerr << "Failed to open dev-topological: " << zx_status_get_string(status) << '\n';
    return zx::error(status);
  }
  fbl::unique_fd dev_fd;
  status = fdio_fd_create(dev_client.TakeChannel().release(), dev_fd.reset_and_get_address());
  if (status != ZX_OK) {
    std::cerr << "Failed to create fd: " << zx_status_get_string(status) << '\n';
    return zx::error(status);
  }

  BusLauncher launcher(std::move(realm), std::move(dev_fd));
  launcher.loop_ = std::move(loop);

  status = fdio_watch_directory(
      launcher.GetRootFd(),
      [](int, int event, const char* fn, void*) {
        if (event != WATCH_EVENT_ADD_FILE) {
          return ZX_OK;
        }
        return strcmp("usb-virtual-bus", fn) ? ZX_OK : ZX_ERR_STOP;
      },
      ZX_TIME_INFINITE, nullptr);
  if (status != ZX_ERR_STOP) {
    std::cerr << "Failed to wait for usb-virtual-bus: " << zx_status_get_string(status) << '\n';
    return zx::error(status);
  }

  fdio_cpp::UnownedFdioCaller caller(launcher.devfs_root_);

  auto virtual_bus = component::ConnectAt<fuchsia_hardware_usb_virtual_bus::Bus>(caller.directory(),
                                                                                 "usb-virtual-bus");
  if (virtual_bus.is_error()) {
    std::cerr << "Failed to wait for usb-virtual-bus: " << virtual_bus.status_string() << '\n';
    return virtual_bus.take_error();
  }
  launcher.virtual_bus_.Bind(std::move(*virtual_bus));

  if (zx_status_t status = launcher.Enable(); status != ZX_OK) {
    return zx::error(status);
  }
  fidl::UnownedClientEnd<fuchsia_io::Directory> svc = launcher.GetExposedDir();
  component::SyncServiceMemberWatcher<fuchsia_hardware_usb_peripheral::Service::Device> watcher(
      svc);
  // Wait indefinitely until a service instance appears in the service directory
  zx::result<fidl::ClientEnd<fuchsia_hardware_usb_peripheral::Device>> peripheral =
      watcher.GetNextInstance(false);
  if (peripheral.is_error()) {
    std::cerr << "Failed to get USB peripheral service: " << peripheral.status_string() << '\n';
    return peripheral.take_error();
  }
  launcher.peripheral_.Bind(std::move(peripheral.value()));

  if (zx_status_t status = launcher.ClearPeripheralDeviceFunctions(); status != ZX_OK) {
    std::cerr << "launcher.ClearPeripheralDeviceFunctions(): " << zx_status_get_string(status)
              << '\n';
    return zx::error(status);
  }

  return zx::ok(std::move(launcher));
}

BusLauncher::~BusLauncher() {
  driver_test_realm::ShutdownRealm(realm_);
  if (loop_) {
    loop_->Shutdown();
  }
}

zx_status_t BusLauncher::SetupPeripheralDevice(DeviceDescriptor&& device_desc,
                                               std::vector<ConfigurationDescriptor> config_descs) {
  auto [client, server] = fidl::Endpoints<fuchsia_hardware_usb_peripheral::Events>::Create();

  if (const fidl::Status result = peripheral_->SetStateChangeListener(std::move(client));
      !result.ok()) {
    std::cerr << "peripheral_->SetStateChangeListener(): " << result.FormatDescription() << '\n';
    return result.status();
  }

  const fidl::WireResult result = peripheral_->SetConfiguration(
      device_desc, fidl::VectorView<ConfigurationDescriptor>::FromExternal(config_descs));
  if (!result.ok()) {
    std::cerr << "peripheral_->SetConfiguration(): " << result.FormatDescription() << '\n';
    return result.status();
  }
  const fit::result response = result.value();
  if (response.is_error()) {
    std::cerr << "peripheral_->SetConfiguration(): " << zx_status_get_string(response.error_value())
              << '\n';
    return response.error_value();
  }

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  EventWatcher watcher(loop, std::move(server), 1);

  if (zx_status_t status = loop.Run(); status != ZX_ERR_CANCELED) {
    std::cerr << "loop.Run(): " << zx_status_get_string(status) << '\n';
    return status;
  }
  if (!watcher.all_functions_registered()) {
    std::cerr << "watcher.all_functions_registered() returned false" << '\n';
    return ZX_ERR_INTERNAL;
  }

  auto connect_result = virtual_bus_->Connect();
  if (connect_result.status() != ZX_OK) {
    std::cerr << "virtual_bus_->Connect(): " << zx_status_get_string(connect_result.status())
              << '\n';
    return connect_result.status();
  }
  if (connect_result.value().status != ZX_OK) {
    std::cerr << "virtual_bus_->Connect() returned status: "
              << zx_status_get_string(connect_result.value().status) << '\n';
    return connect_result.value().status;
  }
  return ZX_OK;
}

zx_status_t BusLauncher::ClearPeripheralDeviceFunctions() {
  if (!peripheral_.is_valid()) {
    return ZX_ERR_INTERNAL;
  }
  auto [client, server] = fidl::Endpoints<fuchsia_hardware_usb_peripheral::Events>::Create();

  if (const fidl::Status result = peripheral_->SetStateChangeListener(std::move(client));
      !result.ok()) {
    std::cerr << "peripheral_->SetStateChangeListener(): " << result.FormatDescription() << '\n';
    return result.status();
  }

  const fidl::WireResult result = peripheral_->ClearFunctions();
  if (!result.ok()) {
    std::cerr << "peripheral_->ClearFunctions(): " << result.FormatDescription() << '\n';
    return result.status();
  }

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  EventWatcher watcher(loop, std::move(server), 1);

  if (zx_status_t status = loop.Run(); status != ZX_ERR_CANCELED) {
    std::cerr << "loop.Run(): " << zx_status_get_string(status) << '\n';
    return status;
  }
  if (!watcher.all_functions_cleared()) {
    std::cerr << "watcher.all_functions_cleared() returned false" << '\n';
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

int BusLauncher::GetRootFd() { return devfs_root_.get(); }

fidl::UnownedClientEnd<fuchsia_io::Directory> BusLauncher::GetExposedDir() {
  return fidl::UnownedClientEnd<fuchsia_io::Directory>(
      realm_.component().exposed().unowned_channel()->get());
}

zx_status_t BusLauncher::Disable() {
  auto result = virtual_bus_->Disable();
  if (result.status() != ZX_OK) {
    std::cerr << "virtual_bus_->Disable(): " << zx_status_get_string(result.status()) << '\n';
    return result.status();
  }
  if (result.value().status != ZX_OK) {
    std::cerr << "virtual_bus_->Disable() returned status: "
              << zx_status_get_string(result.value().status) << '\n';
    return result.value().status;
  }
  return result.value().status;
}

zx_status_t BusLauncher::Enable() {
  auto result = virtual_bus_->Enable();
  if (!result.ok()) {
    std::cerr << "virtual_bus_->Enable(): " << result.FormatDescription() << '\n';
    return result.status();
  }
  if (zx_status_t status = result.value().status; status != ZX_OK) {
    std::cerr << "virtual_bus_->Enable() returned status: " << zx_status_get_string(status) << '\n';
    return status;
  }
  return ZX_OK;
}

zx_status_t BusLauncher::Disconnect() {
  auto result = virtual_bus_->Disconnect();
  if (result.status() != ZX_OK) {
    std::cerr << "virtual_bus_->Disconnect(): " << zx_status_get_string(result.status()) << '\n';
    return result.status();
  }
  if (result.value().status != ZX_OK) {
    std::cerr << "virtual_bus_->Disconnect() returned status: "
              << zx_status_get_string(result.value().status) << '\n';
    return result.value().status;
  }
  return ZX_OK;
}

}  // namespace usb_virtual
