// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/cobalt/bin/app/current_channel_provider.h"

#include <fidl/fuchsia.update.channel/cpp/markers.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/syslog/cpp/macros.h>

namespace cobalt {

namespace {

class EventHandler : public fidl::AsyncEventHandler<fuchsia_update_channel::Provider> {
 public:
  void on_fidl_error(fidl::UnbindInfo error) override { FX_LOGS(ERROR) << error; }
};

}  // namespace

CurrentChannelProvider::CurrentChannelProvider(
    async_dispatcher_t* dispatcher, fidl::ClientEnd<fuchsia_update_channel::Provider> client_end,
    inspect::Node inspect_node, const std::string& current_channel)
    : event_handler_(std::make_unique<EventHandler>()),
      client_(std::move(client_end), dispatcher, event_handler_.get()),
      inspect_node_(std::move(inspect_node)),
      channel_(inspect_node_.CreateString("channel", current_channel)) {}

void CurrentChannelProvider::GetCurrentChannel(fit::callback<void(const std::string&)> callback) {
  client_->GetCurrent().Then(
      [this, callback = std::move(callback)](
          fidl::Result<fuchsia_update_channel::Provider::GetCurrent>& result) mutable {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "Failed to retrieve current channel: " << result.error_value();
          return;
        }

        FX_LOGS(INFO) << "Setting channel to `" << result->channel() << "`";
        channel_.Set(result->channel());
        callback(result->channel());
      });
}

}  // namespace cobalt
