// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/internal/start_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/inspect/component/cpp/component.h>

namespace fdf {

DriverContext::DriverContext(fuchsia_driver_framework::DriverStartArgs start_args)
    : start_args_(std::move(start_args)) {
  incoming_ = std::make_unique<Namespace>([ns = std::move(start_args_.incoming())]() mutable {
    ZX_ASSERT(ns.has_value());
    zx::result incoming = Namespace::Create(ns.value());
    ZX_ASSERT_MSG(incoming.is_ok(), "%s", incoming.status_string());
    return std::move(incoming.value());
  }());

  zx::result val = fdf_internal::ProgramValue(program(), "service_connect_validation");
  if (val.is_ok() && val.value() == "true") {
    EnableServiceValidator();
  }
}

void DriverContext::EnableServiceValidator() {
  if (start_args_.node_offers().has_value()) {
    incoming_->SetServiceValidator(
        std::make_optional<ServiceValidator>(start_args_.node_offers().value()));
  } else {
    fdf::info("No node_offers available, not able to enable service validation.");
  }
}

inspect::ComponentInspector DriverContext::CreateInspector(DriverBase2* driver,
                                                           inspect::Inspector inspector) const {
  return inspect::ComponentInspector(
      driver->dispatcher(),
      inspect::PublishOptions{
          .inspector = std::move(inspector),
          .tree_name = {std::string(driver->name())},
          .client_end = incoming().Connect<fuchsia_inspect::InspectSink>().value(),
      });
}

cpp20::span<const fuchsia_driver_framework::NodeProperty2> DriverContext::node_properties(
    const std::string& parent_node_name) const {
  const auto& node_properties = start_args_.node_properties_2();
  if (node_properties.has_value()) {
    for (const auto& entry : node_properties.value()) {
      if (entry.name() == parent_node_name) {
        return entry.properties();
      }
    }
  }
  return {};
}

zx::event DriverContext::take_node_token() {
  ZX_ASSERT(start_args_.node_token().has_value());
  return *std::move(start_args_.node_token());
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>>
DriverContext::take_power_element_runner() {
  if (!start_args_.power_element_args().has_value()) {
    return std::nullopt;
  }

  return fidl::ServerEnd<fuchsia_power_broker::ElementRunner>(
      start_args_.power_element_args()->runner_server()->TakeChannel());
}

std::optional<fidl::ClientEnd<fuchsia_power_broker::Lessor>>
DriverContext::take_power_element_lessor() {
  if (!start_args_.power_element_args().has_value()) {
    return std::nullopt;
  }

  return fidl::ClientEnd<fuchsia_power_broker::Lessor>(
      std::move(start_args_.power_element_args()->lessor_client().value()));
}

std::optional<fuchsia_power_broker::DependencyToken> DriverContext::power_element_token() {
  if (!start_args_.power_element_args().has_value()) {
    return std::nullopt;
  }

  zx::event copy;
  ZX_ASSERT(start_args_.power_element_args()->token()->duplicate(ZX_RIGHT_SAME_RIGHTS, &copy) ==
            ZX_OK);

  return fuchsia_power_broker::DependencyToken(std::move(copy));
}

bool DriverContext::has_power_args() { return start_args_.power_element_args().has_value(); }
#endif

DriverBase2::DriverBase2(std::string_view name)
    : name_(name), driver_dispatcher_(fdf_dispatcher_get_current_dispatcher()) {}

void DriverBase2::DriverBaseInternalInit(DriverContext& context,
                                         fdf::UnownedSynchronizedDispatcher driver_dispatcher) {
  node_ = std::move(context.start_args_.node().value());
  driver_dispatcher_ = std::move(driver_dispatcher);
  logger_ = Logger::Create2(context.incoming(), dispatcher(), name_, FUCHSIA_LOG_INFO
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
                            ,
                            std::move(context.start_args_.log_sink())
#endif
  );
  Logger::SetGlobalInstance(logger_.get());
  std::optional outgoing_request = std::move(context.start_args_.outgoing_dir());
  ZX_ASSERT(outgoing_request.has_value());
  outgoing_ =
      std::make_shared<OutgoingDirectory>(OutgoingDirectory::Create(driver_dispatcher_->get()));
  ZX_ASSERT(outgoing_->Serve(std::move(outgoing_request.value())).is_ok());
}

zx::result<OwnedChildNode> DriverBase2::AddOwnedChild(std::string_view node_name) {
  return fdf::AddOwnedChild(node(), logger(), node_name);
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> DriverBase2::AddChild(
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  return fdf::AddChild(node(), logger(), node_name, properties, offers);
}

zx::result<OwnedChildNode> DriverBase2::AddOwnedChild(
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args) {
  return fdf::AddOwnedChild(node(), logger(), node_name, devfs_args);
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> DriverBase2::AddChild(
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
    cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  return fdf::AddChild(node(), logger(), node_name, devfs_args, properties, offers);
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> DriverBase2::AddChild(
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  return fdf::AddChild(node(), logger(), node_name, properties, offers);
}

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> DriverBase2::AddChild(
    std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  return fdf::AddChild(node(), logger(), node_name, devfs_args, properties, offers);
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
zx::result<fidl::ClientEnd<fuchsia_driver_framework::ResourceController>>
DriverBase2::ProvideResource(std::string_view node_name,
                             cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
                             cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  return fdf::ProvideResource(node(), logger(), node_name, properties, offers);
}
#endif

DriverBase2::~DriverBase2() { Logger::SetGlobalInstance(nullptr); }

}  // namespace fdf
