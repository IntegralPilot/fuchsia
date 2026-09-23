// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples.metadata/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata_server.h>

#include <bind/fuchsia_examples_metadata_bind_library/cpp/bind.h>

namespace examples::drivers::metadata {

// This driver demonstrates how to send
// `fuchsia.examples.metadata.Metadata` metadata to its children.
// It implements `fuchsia_examples_metadata::Sender` protocol for testing.
class SenderDriver final : public fdf::DriverBase2,
                           public fidl::Server<fuchsia_examples_metadata::Sender> {
 public:
  SenderDriver() : DriverBase2("sender") {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override {
    zx::result result = outgoing()->AddService<fuchsia_examples_metadata::SenderService>(
        fuchsia_examples_metadata::SenderService::InstanceHandler(
            {.device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure)}));
    if (result.is_error()) {
      fdf::error("Failed to add service: {}", result);
      return result.take_error();
    }

    return zx::ok();
  }

  // fidl::Server<fuchsia_examples_metadata::Sender> implementation.
  void ServeMetadata(ServeMetadataRequest& request,
                     ServeMetadataCompleter::Sync& completer) override {
    if (child_.has_value()) {
      fdf::error("Already serving metadata");
      completer.Reply(zx::error(ZX_ERR_BAD_STATE));
      return;
    }

    zx::result result = metadata_server_.Serve(*outgoing(), dispatcher(), request.metadata());
    if (result.is_error()) {
      fdf::error("Failed to set metadata: {}", result);
      completer.Reply(result.take_error());
      return;
    }

    static const std::vector<fuchsia_driver_framework::NodeProperty2> kProperties = {
        fdf::MakeProperty2(bind_fuchsia_examples_metadata::CHILD_TYPE,
                           bind_fuchsia_examples_metadata::CHILD_TYPE_FORWARDER)};

    // Offer the metadata service to the child node.
    std::vector<fuchsia_driver_framework::Offer> offers;
    std::optional metadata_offer = metadata_server_.CreateOffer();
    if (metadata_offer.has_value()) {
      offers.push_back(metadata_offer.value());
    }

    zx::result child = AddChild("sender", kProperties, offers);
    if (child.is_error()) {
      fdf::error("Failed to add child: {}", child);
      completer.Reply(child.take_error());
      return;
    }
    child_.emplace(std::move(child.value()));

    completer.Reply(fit::ok());
  }

 private:
  // Responsible for serving metadata.
  fdf_metadata::MetadataServer<fuchsia_examples_metadata::Metadata> metadata_server_;

  fidl::ServerBindingGroup<fuchsia_examples_metadata::Sender> bindings_;
  std::optional<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> child_;
};

}  // namespace examples::drivers::metadata

FUCHSIA_DRIVER_EXPORT2(examples::drivers::metadata::SenderDriver);
