// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.runtime.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace fcd = fuchsia_component_decl;
namespace ft = fuchsia_runtime_test;

namespace {

class RootDriver : public fdf::DriverBase2, public fidl::WireServer<ft::Handshake> {
 public:
  RootDriver() : fdf::DriverBase2("root") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("Start hook reached");
    node_.Bind(take_node(), dispatcher());
    // Setup the outgoing directory.
    auto handler = ft::Service::InstanceHandler({
        .handshake =
            [this](fidl::ServerEnd<ft::Handshake> server_end) {
              fidl::BindServer(dispatcher(), std::move(server_end), this);
            },
    });
    zx::result<> status = outgoing()->AddService<ft::Service>(std::move(handler));
    if (status.is_error()) {
      return status;
    }

    // Offer `fuchsia.test.Handshake` to the driver that binds to the node.
    auto offer = fdf::MakeOffer2<ft::Service>();

    // Set the properties of the node that a driver will bind to.
    auto property =
        fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_DEVICE);

    auto args = fdf::NodeAddArgs{{
        .name = "leaf",
        .offers2 = {{offer}},
        .properties2 = {{property}},
    }};

    // Create endpoints of the `NodeController` for the node.
    auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }

    node_->AddChild({std::move(args), std::move(endpoints->server), {}})
        .Then([this, client = std::move(endpoints->client)](
                  fidl::Result<fdf::Node::AddChild>& add_result) mutable {
          if (add_result.is_error()) {
            fdf::error("Failed to AddChild: {}",
                       add_result.error_value().FormatDescription().c_str());
            node_.AsyncTeardown();
            return;
          }

          controller_.Bind(std::move(client), dispatcher());
        });

    return zx::ok();
  }

 private:
  // fidl::WireServer<ft::Handshake>
  void Do(DoCompleter::Sync& completer) override { completer.Reply(); }

  fidl::SharedClient<fdf::Node> node_;
  fidl::SharedClient<fdf::NodeController> controller_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
