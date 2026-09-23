// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.compat.runtime.test/cpp/driver/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/compat/cpp/symbols.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_compat_runtime_test;

namespace {

const std::string_view kChildName = "v1";

class RootDriver : public fdf::DriverBase2, public fdf::Server<ft::Root> {
 public:
  RootDriver() : fdf::DriverBase2("root") {}

  static constexpr const char* Name() { return "root"; }

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_ = context.take_incoming();
    node_name_ = context.node_name().value_or("");

    node_.Bind(take_node(), dispatcher());

    // Setup the outgoing directory.
    zx::result outgoing_result = outgoing()->AddService<ft::Service>(
        ft::Service::InstanceHandler({
            .root = bindings_.CreateHandler(this, driver_dispatcher()->get(),
                                            fidl::kIgnoreBindingClosure),
        }),
        kChildName);
    if (outgoing_result.is_error()) {
      fdf::error("Failed to add service {}", outgoing_result);
      return outgoing_result.take_error();
    }
    // Start the driver.
    auto result = AddChild();
    if (result.is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok();
  }

  // fdf::Server<ft::Root>
  void GetString(GetStringCompleter::Sync& completer) override {
    char str[100];
    strcpy(str, "hello world!");
    completer.Reply(std::string(str));
  }

 private:
  fit::result<fdf::NodeError> AddChild() {
    fidl::Arena arena;

    auto offer = fdf::MakeOffer2<ft::Service>(kChildName);

    // Set the properties of the node that a driver will bind to.
    auto property =
        fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD);

    auto args = fdf::NodeAddArgs{{
        .name = std::string(kChildName),
        .offers2 = std::vector{std::move(offer)},
        .properties2 = std::vector{std::move(property)},
    }};

    // Create endpoints of the `NodeController` for the node.
    auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
    if (endpoints.is_error()) {
      return fit::error(fdf::NodeError::kInternal);
    }

    auto add_result = node_.sync()->AddChild(fidl::ToWire(arena, std::move(args)),
                                             std::move(endpoints->server), {});
    if (!add_result.ok()) {
      return fit::error(fdf::NodeError::kInternal);
    }
    if (add_result->is_error()) {
      return fit::error(add_result->error_value());
    }
    controller_.Bind(std::move(endpoints->client), dispatcher());
    return fit::ok();
  }

  fidl::WireClient<fdf::Node> node_;
  fidl::WireSharedClient<fdf::NodeController> controller_;

  fdf::ServerBindingGroup<ft::Root> bindings_;

  std::unique_ptr<fdf::Namespace> incoming_;
  std::string node_name_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
