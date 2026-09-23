// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.examples.routing.echo/cpp/fidl.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/async-helpers/cpp/task_group.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <zircon/assert.h>

#include <gtest/gtest.h>

class Server : public fidl::Server<fidl_examples_routing_echo::Echo> {
 public:
  explicit Server(fidl::ServerEnd<fidl_examples_routing_echo::Echo> server_end)
      : binding_(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server_end), this,
                 fidl::kIgnoreBindingClosure) {}

  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(request.value());
  }

  // Helps ensure the server binding has been created so we don't shutdown the dispatcher too soon.
  void Sync() {}

 private:
  fidl::ServerBinding<fidl_examples_routing_echo::Echo> binding_;
};

TEST(AsyncHelpersTest, AddedToTaskGroup) {
  auto [client_end, server_end] = fidl::Endpoints<fidl_examples_routing_echo::Echo>::Create();

  fdf_testing::DriverRuntime runtime;
  fdf::UnownedSynchronizedDispatcher bg_dispatcher = runtime.StartBackgroundDispatcher();
  async_patterns::TestDispatcherBound<Server> server(bg_dispatcher->async_dispatcher(),
                                                     std::in_place, std::move(server_end));
  server.SyncCall(&Server::Sync);

  fdf::async_helpers::TaskGroup tasks;
  {
    fidl::Client<fidl_examples_routing_echo::Echo> client(
        std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

    fdf::async_helpers::AsyncTask async_task;
    client->EchoString({"string"})
        .Then([completer = async_task.CreateCompleter(),
               quit = runtime.QuitClosure()](auto& result) mutable { quit(); });
    async_task.SetItem(std::move(client));
    tasks.AddTask(std::move(async_task));
  }

  // The callback will call quit to finish the Run.
  runtime.Run();
}

TEST(AsyncHelpersTest, DropAsyncTask) {
  auto [client_end, server_end] = fidl::Endpoints<fidl_examples_routing_echo::Echo>::Create();

  fdf_testing::DriverRuntime runtime;
  fdf::UnownedSynchronizedDispatcher bg_dispatcher = runtime.StartBackgroundDispatcher();
  async_patterns::TestDispatcherBound<Server> server(bg_dispatcher->async_dispatcher(),
                                                     std::in_place, std::move(server_end));
  server.SyncCall(&Server::Sync);

  {
    fidl::Client<fidl_examples_routing_echo::Echo> client(
        std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

    fdf::async_helpers::AsyncTask async_task;
    client->EchoString({"string"})
        .Then([tracker = async_task.CreateCompleter()](auto& result) mutable {
          ZX_ASSERT_MSG(false, "This callback should not have ran.");
        });
    async_task.SetItem(std::move(client));
    // The async_task will be dropped here to signal cancellation of the request.
  }

  // Nothing will run since we canceled the request.
  runtime.RunUntilIdle();
}
