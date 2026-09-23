// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.test.dso/cpp/fidl.h>
#include <fidl/fuchsia.process.lifecycle/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/dispatcher.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/compiler.h>

#include "src/lib/dso/cpp/async.h"

namespace {

class TestHelperImpl : public fidl::Server<fidl_test_dso::TestHelper> {
 public:
  void Ping(PingCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "received Ping, replying pong";
    fidl_test_dso::TestHelperPingResponse response;
    response.response("pong");
    completer.Reply(response);
  }
};

class LifecycleHandler : public fidl::Server<fuchsia_process_lifecycle::Lifecycle> {
 public:
  static void Create(zx_handle_t lifecycle, async_dispatcher_t* dispatcher,
                     std::unique_ptr<component::OutgoingDirectory> outgoing,
                     fidl::Client<fidl_test_dso::TestHelper> client) {
    fidl::ServerEnd server_end =
        fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>{zx::channel(lifecycle)};
    ZX_ASSERT(server_end.is_valid());
    new LifecycleHandler(std::move(server_end), dispatcher, std::move(outgoing), std::move(client));
  }

 private:
  LifecycleHandler(fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> server_end,
                   async_dispatcher_t* dispatcher,
                   std::unique_ptr<component::OutgoingDirectory> outgoing,
                   fidl::Client<fidl_test_dso::TestHelper> client)
      : binding_(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure),
        outgoing_(std::move(outgoing)),
        client_(std::move(client)) {}

  void Stop(StopCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "received Stop request";
    binding_.Close(ZX_OK);
    delete this;
  }

  fidl::ServerBinding<fuchsia_process_lifecycle::Lifecycle> binding_;
  std::unique_ptr<component::OutgoingDirectory> outgoing_;
  fidl::Client<fidl_test_dso::TestHelper> client_;
};

}  // namespace

int dso_main_async(int argc, const char** argv, const char** envp, zx_handle_t svc, zx_handle_t pkg,
                   zx_handle_t directory_request, zx_handle_t lifecycle, zx_handle_t config,
                   fdf_dispatcher_t* fdf_dispatcher) {
  FX_LOGS(INFO) << "dso_main_async started";
  async_dispatcher_t* const dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);

  // Post a task to run on the dispatcher thread
  async::PostTask(dispatcher, [dispatcher, directory_request, lifecycle, svc] {
    // Connect to TestHelper in namespace
    fidl::ClientEnd<fuchsia_io::Directory> svc_client_end{zx::channel(svc)};
    auto client_end = component::ConnectAt<fidl_test_dso::TestHelper>(svc_client_end);
    ZX_ASSERT_MSG(client_end.is_ok(), "Failed to connect to TestHelper: %s",
                  client_end.status_string());

    fidl::Client client(std::move(*client_end), dispatcher);

    FX_LOGS(INFO) << "sending ping to mock server";
    client->Ping().Then([dispatcher, directory_request, lifecycle, client = std::move(client)](
                            fidl::Result<fidl_test_dso::TestHelper::Ping>& result) mutable {
      ZX_ASSERT_MSG(result.is_ok(), "Ping failed: %s",
                    result.error_value().FormatDescription().c_str());
      ZX_ASSERT(result->response() == "pong");
      FX_LOGS(INFO) << "ping succeeded";

      auto outgoing = std::make_unique<component::OutgoingDirectory>(dispatcher);
      auto impl = std::make_unique<TestHelperImpl>();

      zx::result<> status = outgoing->AddProtocol<fidl_test_dso::TestHelper>(std::move(impl));
      ZX_ASSERT_MSG(status.is_ok(), "Failed to add protocol: %s", status.status_string());

      fidl::ServerEnd<fuchsia_io::Directory> server_end{zx::channel(directory_request)};
      status = outgoing->Serve(std::move(server_end));
      ZX_ASSERT_MSG(status.is_ok(), "Failed to serve outgoing directory: %s",
                    status.status_string());

      LifecycleHandler::Create(lifecycle, dispatcher, std::move(outgoing), std::move(client));
      FX_LOGS(INFO) << "initialization task completed";
    });
  });

  return 0;
}
