// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/driver/testing/cpp/test_node.h>
#include <lib/fdf/cpp/dispatcher.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
#include <lib/fdio/directory.h>
#endif
#include <zircon/availability.h>

namespace fdf_testing {

namespace {

async_dispatcher_t* GetDefaultDispatcher() {
  async_dispatcher_t* current_fdf_dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  if (current_fdf_dispatcher) {
    return current_fdf_dispatcher;
  }

  return async_get_default_dispatcher();
}

}  // namespace

TestNode::TestNode(std::string name, async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher ? dispatcher : GetDefaultDispatcher()),
      name_(std::move(name)),
      checker_(dispatcher_, "|fdf_testing::TestNode| is thread-unsafe.") {}

TestNode::~TestNode() { std::lock_guard guard(checker_); }

zx::result<fidl::ClientEnd<fuchsia_driver_framework::Node>> TestNode::CreateNodeChannel() {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  if (zx::result result = Serve(std::move(server_end)); result.is_error()) {
    return result.take_error();
  }

  return zx::ok(std::move(client_end));
}

zx::result<> TestNode::Serve(fidl::ServerEnd<fuchsia_driver_framework::Node> server_end) {
  std::lock_guard guard(checker_);
  if (node_binding_.has_value()) {
    RemoveFromParent();
  }

  node_binding_.emplace(dispatcher_, std::move(server_end), this,
                        [this](fidl::UnbindInfo) { RemoveFromParent(); });
  return zx::ok();
}

zx::result<TestNode::CreateStartArgsResult> TestNode::CreateStartArgsAndServe() {
  std::lock_guard guard(checker_);
  auto incoming_directory_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto outgoing_directory_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto incoming_node_endpoints = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  zx::result serve_result = Serve(std::move(incoming_node_endpoints.server));
  if (serve_result.is_error()) {
    return serve_result.take_error();
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  auto [log_sink_client, log_sink_server] = *fidl::CreateEndpoints<fuchsia_logger::LogSink>();
  ZX_ASSERT(fdio_service_connect_at(incoming_directory_endpoints.client.channel().get(),
                                    "svc/fuchsia.logger.LogSink",
                                    log_sink_server.TakeChannel().release()) == ZX_OK);
#endif

  auto incoming_entries = std::vector<fuchsia_component_runner::ComponentNamespaceEntry>(1);
  incoming_entries[0] = fuchsia_component_runner::ComponentNamespaceEntry({
      .path = "/",
      .directory = std::move(incoming_directory_endpoints.client),
  });

  auto start_args = fuchsia_driver_framework::DriverStartArgs({
      .node = std::move(incoming_node_endpoints.client),
      .incoming = std::move(incoming_entries),
      .outgoing_dir = std::move(outgoing_directory_endpoints.server),
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
      .log_sink = std::move(log_sink_client),
#endif
  });

  return zx::ok(CreateStartArgsResult{
      .start_args = std::move(start_args),
      .incoming_directory_server = std::move(incoming_directory_endpoints.server),
      .outgoing_directory_client = std::move(outgoing_directory_endpoints.client),
  });
}

zx::result<zx::channel> TestNode::ConnectToDevice() {
  zx::channel client_end, server_end;
  zx_status_t status = zx::channel::create(0, &client_end, &server_end);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::lock_guard guard(checker_);
  if (!devfs_connector_client_.is_valid()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fidl::OneWayStatus one_way_status = devfs_connector_client_->Connect(std::move(server_end));
  if (!one_way_status.ok()) {
    return zx::error(one_way_status.status());
  }

  return zx::ok(std::move(client_end));
}

void TestNode::AddChild(AddChildRequestView request, AddChildCompleter::Sync& completer) {
  std::lock_guard guard(checker_);
  std::string name{request->args.name().get()};
  auto [it, inserted] = children_.try_emplace(name, name, dispatcher_);
  if (!inserted) {
    completer.ReplyError(fuchsia_driver_framework::NodeError::kNameAlreadyExists);
    return;
  }
  TestNode& node = it->second;

#if FUCHSIA_API_LEVEL_LESS_THAN(27)
  if (request->args.has_properties()) {
    node.SetProperties(fidl::ToNatural(request->args.properties()).value());
  }
#else
  if (request->args.has_properties() && request->args.has_properties2()) {
    completer.ReplyError(fuchsia_driver_framework::NodeError::kUnsupportedArgs);
    return;
  }

  if (request->args.has_properties()) {
    std::vector<fuchsia_driver_framework::NodeProperty2> properties;
    properties.reserve(request->args.properties().size());
    for (auto& deprecated_prop : request->args.properties().get()) {
      if (deprecated_prop.key.Which() !=
          fuchsia_driver_framework::wire::NodePropertyKey::Tag::kStringValue) {
        completer.ReplyError(fuchsia_driver_framework::NodeError::kUnsupportedArgs);
        return;
      }

      properties.emplace_back(fuchsia_driver_framework::NodeProperty2{
          {.key = std::string(deprecated_prop.key.string_value().get()),
           .value = fidl::ToNatural(deprecated_prop.value)}});
    }
    node.SetProperties(properties);
  } else if (request->args.has_properties2()) {
    node.SetProperties(fidl::ToNatural(request->args.properties2()).value());
  }
#endif

  node.SetParent(this, std::move(request->controller));
  if (request->node) {
    zx::result result = node.Serve(std::move(request->node));
    ZX_ASSERT_MSG(result.is_ok(), "|Serve| failed with %s", result.status_string());
  }

  if (request->args.has_devfs_args()) {
    fuchsia_driver_framework::wire::DevfsAddArgs& devfs_args = request->args.devfs_args();
    node.set_devfs_connector_client(std::move(devfs_args.connector()));
  }

  completer.ReplySuccess();
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
void TestNode::ProvideResource(
    fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceRequestView request,
    fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceCompleter::Sync& completer) {
  completer.ReplySuccess();
}
#endif

void TestNode::RequestBind(RequestBindRequestView request, RequestBindCompleter::Sync& completer) {
  std::lock_guard guard(checker_);
  bool force_rebind = false;
  if (request->has_force_rebind()) {
    force_rebind = request->force_rebind();
  }
  std::string driver_url_suffix;
  if (request->has_driver_url_suffix()) {
    driver_url_suffix = std::string(request->driver_url_suffix().get());
  }

  bind_data_.push_back(BindData{
      .force_rebind = force_rebind,
      .driver_url_suffix = std::move(driver_url_suffix),
  });
  if (!children_.empty() && !force_rebind) {
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  completer.ReplySuccess();
}

#if FUCHSIA_API_LEVEL_AT_LEAST(29)
void TestNode::WaitForDriver(WaitForDriverCompleter::Sync& completer) {
  completer.ReplySuccess(
      fuchsia_driver_framework::wire::DriverResult::WithMatchError(ZX_ERR_NOT_FOUND));
}
#endif

void TestNode::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::NodeController> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

void TestNode::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::Node> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

void TestNode::SetParent(TestNode* parent,
                         fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller) {
  std::lock_guard guard(checker_);
  parent_ = *parent;
  controller_binding_.emplace(dispatcher_, std::move(controller), this,
                              fidl::kIgnoreBindingClosure);
}

#if FUCHSIA_API_LEVEL_LESS_THAN(27)
void TestNode::SetProperties(std::vector<fuchsia_driver_framework::NodeProperty> properties) {
  std::lock_guard guard(checker_);
  properties_ = std::move(properties);
}
#else

void TestNode::SetProperties(std::vector<fuchsia_driver_framework::NodeProperty2> properties) {
  std::lock_guard guard(checker_);
  properties_ = std::move(properties);
}
#endif

void TestNode::RemoveFromParent() {
  std::lock_guard guard(checker_);
  children_.clear();

  if (node_binding_.has_value()) {
    node_binding_.value().Close(ZX_OK);
  }

  node_binding_.reset();

  if (controller_binding_.has_value()) {
    controller_binding_.value().Close(ZX_OK);
  }

  controller_binding_.reset();

  if (!parent_.has_value()) {
    return;
  }
  // After this call we are destructed, so don't access anything else.
  parent_.value().get().RemoveChild(name_);
}

void TestNode::RemoveChild(const std::string& name) {
  std::lock_guard guard(checker_);
  size_t count = children_.erase(name);
  ZX_ASSERT_MSG(count == 1, "Should've removed 1 child, removed %ld", count);
}

}  // namespace fdf_testing
