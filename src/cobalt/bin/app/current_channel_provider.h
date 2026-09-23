// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_COBALT_BIN_APP_CURRENT_CHANNEL_PROVIDER_H_
#define SRC_COBALT_BIN_APP_CURRENT_CHANNEL_PROVIDER_H_

#include <fidl/fuchsia.update.channel/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>

#include <memory>
#include <string>

namespace cobalt {

// Asynchronously retrieves the current channel using fuchsia.update.channel/Provider::GetChannel.
class CurrentChannelProvider {
 public:
  CurrentChannelProvider(async_dispatcher_t* dispatcher,
                         fidl::ClientEnd<fuchsia_update_channel::Provider> client_end,
                         inspect::Node inspect_node, const std::string& current_channel);

  // Asynchronously retrieves the current channel and returns it via |callback|.
  void GetCurrentChannel(fit::callback<void(const std::string&)> callback);

 private:
  std::unique_ptr<fidl::AsyncEventHandler<fuchsia_update_channel::Provider>> event_handler_;
  fidl::Client<fuchsia_update_channel::Provider> client_;
  inspect::Node inspect_node_;
  inspect::StringProperty channel_;
};

}  // namespace cobalt

#endif  // SRC_COBALT_BIN_APP_CURRENT_CHANNEL_PROVIDER_H_
