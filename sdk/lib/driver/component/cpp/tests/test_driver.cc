// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/tests/test_driver.h>
#include <lib/driver/logging/cpp/structured_logger.h>

bool g_driver_stopped = false;

void TestDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_name_ = context.node_name().value_or("test_driver");
  node_client_.Bind(take_node(), dispatcher());
  // Delay the completion to simulate an async workload.
  async::PostDelayedTask(
      dispatcher(), [completer = std::move(completer)]() mutable { completer(zx::ok()); },
      zx::msec(100));

  // Make a dispatcher that we purposely release to make sure the test framework can
  // shut it down for us.
  zx::result<fdf::SynchronizedDispatcher> unowned_dispatcher =
      fdf::SynchronizedDispatcher::Create({}, "unowned_dispatcher", [](auto dispatcher) {});
  ZX_ASSERT(unowned_dispatcher.is_ok());
  unowned_dispatcher.value().release();

  // Make a dispatcher that we purposely don't shutdown in our PrepareStop but keep ownership of
  // to make sure the framework can shut it down for us.
  zx::result<fdf::SynchronizedDispatcher> not_shutdown_manually_dispatcher =
      fdf::SynchronizedDispatcher::Create({}, "not_shutdown_manually_dispatcher",
                                          [](auto dispatcher) {});
  ZX_ASSERT(not_shutdown_manually_dispatcher.is_ok());
  not_shutdown_manually_dispatcher_.emplace(std::move(not_shutdown_manually_dispatcher.value()));
}

zx::result<> TestDriver::ExportDevfsNodeSync() {
  fidl::Arena arena;
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector(std::move(connector.value()))
                   .connector_supports(fuchsia_device_fs::ConnectionType::kDevice |
                                       fuchsia_device_fs::ConnectionType::kController)
                   .Build();
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, "devfs_node")
                  .devfs_args(devfs)
                  .Build();

  // Create endpoints of the `NodeController` for the node.
  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  auto node_endpoints = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  fidl::WireResult result = node_client_.sync()->AddChild(
      args, std::move(controller_endpoints.server), std::move(node_endpoints.server));

  if (!result.ok()) {
    fdf::error("Failed to add child: status={}", result.status_string());
    return zx::error(result.status());
  }

  devfs_node_controller_.Bind(std::move(controller_endpoints.client));
  devfs_node_.Bind(std::move(node_endpoints.client));
  return zx::ok();
}

zx::result<> TestDriver::ServeDriverService() {
  zx::result result = outgoing()->AddService<fuchsia_driver_component_test::DriverService>(
      GetInstanceHandlerDriver());
  if (result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> TestDriver::ServeZirconService() {
  zx::result result = outgoing()->AddService<fuchsia_driver_component_test::ZirconService>(
      GetInstanceHandlerZircon());
  if (result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> TestDriver::ValidateIncomingDriverService(std::string_view instance) {
  zx::result driver_connect_result =
      incoming()->Connect<fuchsia_driver_component_test::DriverService::Device>(instance);
  if (driver_connect_result.is_error()) {
    fdf::warn("Couldn't connect to DriverService.");
    return driver_connect_result.take_error();
  }

  fdf::Arena arena('DRVR');
  fdf::WireUnownedResult wire_result =
      fdf::WireCall(driver_connect_result.value()).buffer(arena)->DriverMethod();
  if (!wire_result.ok()) {
    if (instance != component::kDefaultInstance) {
      fdf::warn("Failed to call DriverMethod {}", wire_result.status_string());
    } else {
      fdf::error("Failed to call DriverMethod {}", wire_result.status_string());
    }
    return zx::error(wire_result.status());
  }

  if (wire_result->is_error()) {
    fdf::error("DriverMethod error {}", zx_status_get_string(wire_result.value().error_value()));
    return wire_result.value().take_error();
  }

  return zx::ok();
}

zx::result<> TestDriver::ValidateIncomingZirconService(std::string_view instance) {
  zx::result zircon_connect_result =
      incoming()->Connect<fuchsia_driver_component_test::ZirconService::Device>(instance);
  if (zircon_connect_result.is_error()) {
    fdf::warn("Couldn't connect to ZirconService.");
    return zircon_connect_result.take_error();
  }

  fidl::WireResult wire_result = fidl::WireCall(zircon_connect_result.value())->ZirconMethod();
  if (!wire_result.ok()) {
    if (instance != component::kDefaultInstance) {
      fdf::warn("Failed to call ZirconMethod {}", wire_result.status_string());
    } else {
      fdf::error("Failed to call ZirconMethod {}", wire_result.status_string());
    }
    return zx::error(wire_result.status());
  }

  if (wire_result->is_error()) {
    fdf::error("ZirconMethod error {}", zx_status_get_string(wire_result.value().error_value()));
    return wire_result.value().take_error();
  }

  // Validate the compat service which is a zircon service.
  zx::result<> result = InitSyncCompat();
  if (result.is_error()) {
    fdf::error("Failed to sync init the compat server. {}", result);
    return result.take_error();
  }

  return zx::ok();
}

void TestDriver::Stop(fdf::StopCompleter completer) {
  if (child_controller_.is_valid()) {
    fidl::OneWayStatus status = child_controller_->Remove();
    if (!status.ok()) {
      completer(zx::make_result(status.status()));
      return;
    }

    // The closing of the node controller will trigger on_fidl_error as this class is the event
    // handler. There it will complete the stop completer.
    stop_completer_.emplace(std::move(completer));
  } else {
    // Delay the completion to simulate an async workload.
    async::PostDelayedTask(
        dispatcher(), [completer = std::move(completer)]() mutable { completer(zx::ok()); },
        zx::msec(100));
  }
}

TestDriver::~TestDriver() { g_driver_stopped = true; }

zx::result<> TestDriver::InitSyncCompat() {
  auto result = sync_device_server_.Initialize(incoming(), outgoing(), node_name_, "child",
                                               compat::ForwardMetadata::All());
  return result;
}

void TestDriver::BeginInitAsyncCompat(fit::callback<void(zx::result<>)> completed) {
  async_device_server_.Begin(
      incoming(), outgoing(), node_name_, "child",
      [completed_cb = std::move(completed)](zx::result<> result) mutable { completed_cb(result); },
      compat::ForwardMetadata::All());
}

void TestDriver::CreateChildNodeSync() {
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  fidl::Arena arena;
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, "child")
                  .offers2(sync_device_server_.CreateOffers2(arena))
                  .Build();
  auto result = node_client_.sync()->AddChild(args, std::move(server_end), {});
  ZX_ASSERT(result.ok());
  ZX_ASSERT(result->is_ok());
  sync_added_child_ = true;
  child_controller_.Bind(std::move(client_end), dispatcher(), this);
}

void TestDriver::CreateChildNodeAsync() {
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  fidl::Arena arena;
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, "child")
                  .offers2(async_device_server_.CreateOffers2(arena))
                  .Build();

  node_client_->AddChild(args, std::move(server_end), {})
      .Then([this, client = std::move(client_end)](
                fidl::WireUnownedResult<fuchsia_driver_framework::Node::AddChild>& result) mutable {
        ZX_ASSERT_MSG(result.ok(), "%s", result.FormatDescription().c_str());
        ZX_ASSERT_MSG(result->is_ok(), "%s", result.FormatDescription().c_str());
        async_added_child_ = true;
        child_controller_.Bind(std::move(client), dispatcher(), this);
      });
}

void TestDriver::on_fidl_error(fidl::UnbindInfo error) {
  if (error.status() != ZX_OK) {
    fdf::error("Child controller binding closed with error: {}", error.status_string());
  }

  if (stop_completer_.has_value()) {
    stop_completer_.value()(zx::make_result(error.status()));
    stop_completer_.reset();
  }
}

void TestDriver::handle_unknown_event(
    fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) {}

DriverRegistration StartFailTestDriver::GetDriverRegistration() {
  // Use a custom DriverRegistration to create the DUT. Without this, the 'TestDriver'
  // implementation will be used by default.
  return FUCHSIA_DRIVER_REGISTRATION_V1(
      fdf_internal::DriverServer2<StartFailTestDriver>::initialize,
      fdf_internal::DriverServer2<StartFailTestDriver>::destroy);
}

void StartFailTestDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  completer(zx::error(ZX_ERR_INTERNAL));
}

FUCHSIA_DRIVER_EXPORT2(TestDriver);
