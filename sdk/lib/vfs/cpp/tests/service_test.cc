// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests verify basic functionality of `vfs::Service`. For more comprehensive tests, see
// //src/storage/lib/vfs/cpp and //src/storage/conformance.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/test.placeholders/cpp/test_base.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>
#include <zircon/status.h>

#include <memory>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {

// Sets up a service at root/instance that drops connections after a certain limit is reached.
class ServiceTest : public ::gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    root_ = std::make_unique<vfs::PseudoDir>();
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root_->Serve(fuchsia_io::kPermReadable, std::move(root_server)), ZX_OK);
    root_client_ = std::move(root_client);
  }

  const fidl::ClientEnd<fuchsia_io::Directory>& root_client() { return root_client_; }

  void AddService(std::unique_ptr<vfs::Service> service, std::string_view path) {
    EXPECT_EQ(root_->AddEntry(std::string(path), std::move(service)), ZX_OK);
  }

 private:
  std::unique_ptr<vfs::PseudoDir> root_;
  std::vector<zx::channel> channels_;
  fidl::ClientEnd<fuchsia_io::Directory> root_client_;
};

TEST_F(ServiceTest, Lambda) {
  // Create a service with a callback that counts the number of times connection attempts are made.
  size_t num_connections = 0;
  auto service =
      std::make_unique<vfs::Service>([&num_connections](zx::channel) { ++num_connections; });
  AddService(std::move(service), "lambda");
  // Connect to the service a few times and ensure the lambda was invoked the same amount of times.
  constexpr size_t kConnectionAttempts = 5;
  PerformBlockingWork([this] {
    for (size_t i = 0; i < kConnectionAttempts; ++i) {
      zx::channel client, server;
      ASSERT_EQ(zx::channel::create(0, &client, &server), ZX_OK);
      ASSERT_EQ(fdio_service_connect_at(root_client().channel().get(), "lambda", server.release()),
                ZX_OK);
    }
  });
  RunLoopUntilIdle();
  ASSERT_EQ(num_connections, kConnectionAttempts);
}

class ExampleService : public fidl::testing::TestBase<test_placeholders::Echo> {
  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(request.value());
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    FAIL() << "Not implemented: " << name;
  }
};

TEST_F(ServiceTest, ServiceSharedInstance) {
  ExampleService instance;
  instance.bind_handler(dispatcher());
  auto service = std::make_unique<vfs::Service>(instance.bind_handler(dispatcher()));
  AddService(std::move(service), "example");
}

TEST_F(ServiceTest, ServiceUniqueInstance) {
  auto service =
      std::make_unique<vfs::Service>([this](fidl::ServerEnd<test_placeholders::Echo> server_end) {
        auto instance = std::make_unique<ExampleService>();
        fidl::BindServer(dispatcher(), std::move(server_end), std::move(instance));
      });
  AddService(std::move(service), "example");
}

}  // namespace
