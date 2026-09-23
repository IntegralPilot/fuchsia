// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_CODECS_DA7219_DA7219_DFV2_H_
#define SRC_MEDIA_AUDIO_DRIVERS_CODECS_DA7219_DA7219_DFV2_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.audio/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <algorithm>
#include <memory>

#include "src/media/audio/drivers/codecs/da7219/da7219-server.h"

namespace audio::da7219 {

// CodecConnector is a service-hub/trampoline mechanism to allow DFv1 Codec drivers to service
// FIDL outside DFv1, not needed in DFv2 but still in used by all DFv1 drivers and clients.
// ServerConnector allows serving CodecConnector FIDL providing the trampoline and also
// allows binding the server directly via BindServer.
class ServerConnector : public fidl::WireServer<fuchsia_hardware_audio::CodecConnector> {
 public:
  explicit ServerConnector(std::shared_ptr<Core> core, bool is_input)
      : core_(core),
        is_input_(is_input),
        devfs_connector_(fit::bind_member<&ServerConnector::BindConnector>(this)) {}

  // Bind the trampoline server.
  void BindConnector(fidl::ServerEnd<fuchsia_hardware_audio::CodecConnector> server) {
    auto on_unbound = [](ServerConnector*, fidl::UnbindInfo info,
                         fidl::ServerEnd<fuchsia_hardware_audio::CodecConnector> server_end) {
      if (info.is_peer_closed()) {
        DA7219_LOG(DEBUG, "Client disconnected");
      } else if (!info.is_user_initiated() && info.status() != ZX_ERR_CANCELED) {
        // Do not log canceled cases which happens too often in particular in test cases.
        DA7219_LOG(ERROR, "Client connection unbound: %s", info.status_string());
      }
    };
    fidl::BindServer(core_->dispatcher(), std::move(server), this, std::move(on_unbound));
  }

  zx::result<> Serve(const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent) {
    fidl::Arena arena;
    zx::result connector = devfs_connector_.Bind(core_->dispatcher());
    if (connector.is_error()) {
      return connector.take_error();
    }

    auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                     .connector(std::move(connector.value()))
                     .class_name("codec");

    auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                    .name(arena, is_input_ ? "da7219-input" : "da7219-output")
                    .devfs_args(devfs.Build())
                    .Build();

    // Create endpoints of the `NodeController` for the node.
    auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
    ZX_ASSERT_MSG(node_endpoints.is_ok(), "Failed: %s", node_endpoints.status_string());

    fidl::WireResult result = fidl::WireCall(parent)->AddChild(
        args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
    if (!result.ok()) {
      FDF_SLOG(ERROR, "Failed to add child", KV("status", result.status_string()));
      return zx::error(result.status());
    }
    controller_.Bind(std::move(controller_endpoints.client));
    node_.Bind(std::move(node_endpoints->client));

    return zx::ok();
  }

  driver_devfs::Connector<fuchsia_hardware_audio::CodecConnector>& devfs_connector() {
    return devfs_connector_;
  }

 private:
  // Bind the server without the trampoline.
  void BindServer(fidl::ServerEnd<fuchsia_hardware_audio::Codec> request) {
    auto on_unbound = [this](fidl::WireServer<fuchsia_hardware_audio::Codec>*,
                             fidl::UnbindInfo info,
                             fidl::ServerEnd<fuchsia_hardware_audio::Codec> server_end) {
      if (info.is_peer_closed()) {
        DA7219_LOG(DEBUG, "Client disconnected");
      } else if (!info.is_user_initiated() && info.status() != ZX_ERR_CANCELED) {
        // Do not log canceled cases which happens too often in particular in test cases.
        DA7219_LOG(ERROR, "Client connection unbound: %s", info.status_string());
      }
      server_.reset();  // Allow re-connecting after unbind.
    };
    server_ = std::make_unique<Server>(core_, is_input_);
    fidl::BindServer(core_->dispatcher(), std::move(request), server_.get(), std::move(on_unbound));
  }

  // LLCPP implementation for the CodecConnector API.
  void Connect(fuchsia_hardware_audio::wire::CodecConnectorConnectRequest* request,
               ConnectCompleter::Sync& completer) override {
    if (server_) {
      completer.Close(ZX_ERR_NO_RESOURCES);  // Only allow one connection.
      return;
    }
    BindServer(std::move(request->codec_protocol));
  }

  std::shared_ptr<Core> core_;
  bool is_input_;
  std::unique_ptr<Server> server_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  driver_devfs::Connector<fuchsia_hardware_audio::CodecConnector> devfs_connector_;
};

class Driver : public fdf::DriverBase2 {
 public:
  Driver() : fdf::DriverBase2("da7219") {}

  ~Driver() override = default;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  zx::result<> Serve(std::string_view name, bool is_input);
  zx::result<zx::interrupt> GetIrq(const fdf::Namespace& incoming) const;

  std::shared_ptr<Core> core_;
  std::shared_ptr<ServerConnector> server_output_;
  std::shared_ptr<ServerConnector> server_input_;
};

}  // namespace audio::da7219

#endif  // SRC_MEDIA_AUDIO_DRIVERS_CODECS_DA7219_DA7219_DFV2_H_
