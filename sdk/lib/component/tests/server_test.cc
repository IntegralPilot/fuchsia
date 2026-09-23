// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fidl/fidl.service.test/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/outgoing/cpp/handlers.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/zx/channel.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "echo_server.h"

class ServerTest : public testing::Test {
 protected:
  ServerTest()
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        outgoing_(loop_.dispatcher()),
        default_foo_("default-foo", loop_.dispatcher()),
        default_bar_("default-bar", loop_.dispatcher()),
        other_foo_("other-foo", loop_.dispatcher()),
        other_bar_("other-bar", loop_.dispatcher()) {}

  component::ServiceInstanceHandler SetUpInstance(fidl::WireServer<Echo>* foo_impl,
                                                  fidl::WireServer<Echo>* bar_impl) {
    EchoService::InstanceHandler handler(
        {.foo = [this, foo_impl](fidl::ServerEnd<Echo> request_channel) -> void {
           fidl::BindServer(loop_.dispatcher(), std::move(request_channel), foo_impl);
         },
         .bar = [this, bar_impl](fidl::ServerEnd<Echo> request_channel) -> void {
           fidl::BindServer(loop_.dispatcher(), std::move(request_channel), bar_impl);
         }});

    return handler;
  }

  void SetUp() override {
    ASSERT_EQ(ZX_OK, outgoing_.AddService<EchoService>(SetUpInstance(&default_foo_, &default_bar_))
                         .status_value());
    ASSERT_EQ(ZX_OK,
              outgoing_.AddService<EchoService>(SetUpInstance(&other_foo_, &other_bar_), "other")
                  .status_value());

    zx::result server = fidl::CreateEndpoints(&local_root_);
    ASSERT_EQ(ZX_OK, server.status_value());

    ASSERT_EQ(ZX_OK, outgoing_.Serve(std::move(server.value())).status_value());
  }

  async::Loop& loop() { return loop_; }
  async_dispatcher_t* dispatcher() { return loop_.dispatcher(); }

  async::Loop loop_;
  fidl::ClientEnd<fuchsia_io::Directory> local_root_;
  component::OutgoingDirectory outgoing_;
  EchoCommon default_foo_;
  EchoCommon default_bar_;
  EchoCommon other_foo_;
  EchoCommon other_bar_;
};

TEST_F(ServerTest, ConnectsToDefaultMember) {
  auto svc_local = component::OpenDirectoryAt(local_root_, "svc");
  ASSERT_EQ(ZX_OK, svc_local.status_value());

  // Connect to the `EchoService` at the 'default' instance.
  zx::result<EchoService::ServiceClient> open_result =
      component::OpenServiceAt<EchoService>(*svc_local);
  ASSERT_TRUE(open_result.is_ok());

  EchoService::ServiceClient service = std::move(open_result.value());

  // Connect to the member 'foo'.
  zx::result<fidl::ClientEnd<Echo>> connect_result = service.connect_foo();
  ASSERT_TRUE(connect_result.is_ok());

  fidl::WireClient client{std::move(connect_result.value()), dispatcher()};
  client->EchoString(fidl::StringView("hello"))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<Echo::EchoString>& echo_result) {
        ASSERT_TRUE(echo_result.ok()) << echo_result.error().FormatDescription();
        auto response = echo_result.Unwrap();
        std::string result_string(response->response.data(), response->response.size());
        ASSERT_EQ(result_string, "default-foo: hello");
        loop().Quit();
      });

  loop().Run();
}

TEST_F(ServerTest, ConnectsToOtherMember) {
  auto svc_local = component::OpenDirectoryAt(local_root_, "svc");
  ASSERT_EQ(ZX_OK, svc_local.status_value());

  // Connect to the `EchoService` at the 'other' instance.
  zx::result<EchoService::ServiceClient> open_result =
      component::OpenServiceAt<EchoService>(*svc_local, "other");
  ASSERT_TRUE(open_result.is_ok());

  EchoService::ServiceClient service = std::move(open_result.value());

  // Connect to the member 'foo'.
  zx::result<fidl::ClientEnd<Echo>> connect_result = service.connect_foo();
  ASSERT_TRUE(connect_result.is_ok());

  fidl::WireClient client{std::move(connect_result.value()), dispatcher()};

  client->EchoString(fidl::StringView("hello"))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<Echo::EchoString>& echo_result) {
        ASSERT_TRUE(echo_result.ok()) << echo_result.error().FormatDescription();
        auto response = echo_result.Unwrap();
        std::string result_string(response->response.data(), response->response.size());
        ASSERT_EQ(result_string, "other-foo: hello");
        loop().Quit();
      });
  loop().Run();
}

TEST_F(ServerTest, ListsMembers) {
  // The POSIX/fd-based APIs are blocking, hence run on a separate thread.
  std::thread t([&] {
    auto defer_quit_loop = fit::defer([&] { loop().Quit(); });

    // Open the 'default' instance of the test service and enumerate its contents.
    auto default_instance =
        component::OpenDirectoryAt(local_root_, "/svc/fidl.service.test.EchoService/default");
    ASSERT_EQ(ZX_OK, default_instance.status_value());

    fbl::unique_fd instance_fd;
    ASSERT_EQ(ZX_OK, fdio_fd_create(default_instance->TakeChannel().release(),
                                    instance_fd.reset_and_get_address()));
    DIR* dir = fdopendir(instance_fd.release());
    ASSERT_NE(dir, nullptr);
    auto defer_closedir = fit::defer([dir] { closedir(dir); });

    dirent* entry = readdir(dir);
    ASSERT_NE(entry, nullptr);
    ASSERT_EQ(std::string(entry->d_name), ".");

    entry = readdir(dir);
    ASSERT_NE(entry, nullptr);
    ASSERT_EQ(std::string(entry->d_name), "bar");

    entry = readdir(dir);
    ASSERT_NE(entry, nullptr);
    ASSERT_EQ(std::string(entry->d_name), "foo");

    ASSERT_EQ(readdir(dir), nullptr);
  });

  loop().Run();
  t.join();
}
