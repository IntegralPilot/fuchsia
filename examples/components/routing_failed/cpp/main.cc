// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.examples.routing.echo/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdlib>
#include <iostream>
#include <string>

class EventHandler : public fidl::AsyncEventHandler<fidl_examples_routing_echo::Echo> {
 public:
  void on_fidl_error(fidl::UnbindInfo error) override {
    FX_LOG_KV(WARNING, "Protocol failed", FX_KV("status", error.status()));
    ZX_ASSERT(error.status() == expected_status_);
    loop_.Quit();
  }
  EventHandler(async::Loop& loop, zx_status_t expected_status)
      : loop_(loop), expected_status_(expected_status) {}

 private:
  async::Loop& loop_;
  zx_status_t expected_status_;
};

int main(int argc, const char* argv[], char* envp[]) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"echo_client"}).BuildAndInitialize();
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  // Connect to the fidl.examples.routing.Echo protocol
  auto client_end = component::Connect<fidl_examples_routing_echo::Echo>();
  ZX_ASSERT(client_end.is_ok());

  EventHandler event_handler(loop, ZX_ERR_UNAVAILABLE);
  fidl::Client echo_proxy(std::move(*client_end), loop.dispatcher(), &event_handler);

  // The `echo` channel should be closed with an epitaph because routing failed (see
  // echo_realm.cml)
  //
  // The epitaph itself is just a zx_status_t. To get detailed information about why the routing
  // failed, you'll need to check the kernel debuglog.
  echo_proxy->EchoString({"Hippos rule!"})
      .ThenExactlyOnce([&](fidl::Result<fidl_examples_routing_echo::Echo::EchoString>& result) {
        if (result.is_ok()) {
          FX_LOG_KV(INFO, "Server response (unexpected)",
                    FX_KV("response", result->response().value().c_str()));
        } else {
          FX_LOG_KV(INFO, "Call failed as expected",
                    FX_KV("error", result.error_value().status_string()));
        }
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();

  // Connect to the fidl.examples.routing.Echo2 protocol
  auto client_end2 =
      component::Connect<fidl_examples_routing_echo::Echo>("fidl.examples.routing.echo.Echo2");
  ZX_ASSERT(client_end2.is_ok());

  EventHandler event_handler2(loop, ZX_ERR_PEER_CLOSED);
  fidl::Client echo2_proxy(std::move(*client_end2), loop.dispatcher(), &event_handler2);

  // The `echo2` channel should be closed because routing succeeded but the runner failed to
  // start the component. The channel won't have an epitaph set; the runner closes the source
  // component's outgoing directory request handle and that causes the channel for the service
  // connection to be closed as well.
  echo2_proxy->EchoString({"Hippos rule!"})
      .ThenExactlyOnce([&](fidl::Result<fidl_examples_routing_echo::Echo::EchoString>& result) {
        if (result.is_ok()) {
          FX_LOG_KV(INFO, "Server response (unexpected)",
                    FX_KV("response", result->response().value().c_str()));
        } else {
          FX_LOG_KV(INFO, "Call failed as expected",
                    FX_KV("error", result.error_value().status_string()));
        }
        loop.Quit();
      });

  loop.Run();
  return 0;
}
