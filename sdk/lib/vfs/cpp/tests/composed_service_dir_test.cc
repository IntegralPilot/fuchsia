// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/vfs/cpp/composed_service_dir.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>
#include <zircon/status.h>

#include <memory>
#include <string>
#include <string_view>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {

constexpr std::string_view kServices[]{"service_a", "service_b"};
constexpr std::string_view kFallbackServices[]{"service_b", "service_c"};
constexpr size_t kServiceConnections = 5;
constexpr size_t kFallbackServiceConnections = 2;

// Fixture sets up a composed service directory containing "service_a" and "service_b" which is
// backed by a fallback directory containing "service_b" and "service_c".
class ComposedServiceDirTest : public ::gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    root_ = std::make_unique<vfs::ComposedServiceDir>();

    for (const auto service : kServices) {
      root_->AddService(std::string(service),
                        std::make_unique<vfs::Service>(MakeService(service, /*fallback*/ false)));
    }

    fallback_dir_ = std::make_unique<vfs::PseudoDir>();
    for (const auto service : kFallbackServices) {
      fallback_dir_->AddEntry(std::string(service), std::make_unique<vfs::Service>(
                                                        MakeService(service, /*fallback*/ true)));
    }

    auto [fallback_client, fallback_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(fallback_dir_->Serve(fuchsia_io::kPermReadable, std::move(fallback_server)), ZX_OK);
    root_->SetFallback(std::move(fallback_client));

    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root_->Serve(fuchsia_io::kPermReadable, std::move(root_server)), ZX_OK);
    root_client_ = std::move(root_client);
  }

  const fidl::ClientEnd<fuchsia_io::Directory>& root_client() { return root_client_; }

  auto& connection_attempts() { return connection_attempts_; }
  auto& fallback_attempts() { return fallback_attempts_; }

  vfs::ComposedServiceDir* root() { return root_.get(); }

 private:
  fit::function<void(zx::channel channel, async_dispatcher_t* dispatcher)> MakeService(
      std::string_view name, bool fallback) {
    return [this, name, fallback](zx::channel channel, async_dispatcher_t* dispatcher) {
      if (fallback) {
        fallback_attempts_[name]++;
      } else {
        connection_attempts_[name]++;
      }
    };
  }

  std::unique_ptr<vfs::ComposedServiceDir> root_;
  std::unique_ptr<vfs::PseudoDir> fallback_dir_;
  std::map<std::string_view, size_t, std::less<>> connection_attempts_;
  std::map<std::string_view, size_t, std::less<>> fallback_attempts_;
  fidl::ClientEnd<fuchsia_io::Directory> root_client_;
};

TEST_F(ComposedServiceDirTest, Connect) {
  PerformBlockingWork([this] {
    for (size_t i = 0; i < kServiceConnections; ++i) {
      for (const auto& service : kServices) {
        zx::channel client, server;
        ASSERT_EQ(zx::channel::create(0, &client, &server), ZX_OK);
        ASSERT_EQ(fdio_service_connect_at(root_client().channel().get(), service.data(),
                                          server.release()),
                  ZX_OK);
      }
    }
    for (size_t i = 0; i < kFallbackServiceConnections; ++i) {
      for (const auto& service : kFallbackServices) {
        zx::channel client, server;
        ASSERT_EQ(zx::channel::create(0, &client, &server), ZX_OK);
        ASSERT_EQ(fdio_service_connect_at(root_client().channel().get(), service.data(),
                                          server.release()),
                  ZX_OK);
      }
    }
  });

  RunLoopUntilIdle();

  // service_a is only in the root, service_b is in both, and service_c is only in the fallback.

  EXPECT_EQ(connection_attempts()["service_a"], kServiceConnections);
  EXPECT_EQ(connection_attempts()["service_b"], kServiceConnections + kFallbackServiceConnections);
  EXPECT_EQ(connection_attempts()["service_c"], 0u);

  EXPECT_EQ(fallback_attempts()["service_a"], 0u);
  EXPECT_EQ(fallback_attempts()["service_b"], 0u);
  EXPECT_EQ(fallback_attempts()["service_c"], kFallbackServiceConnections);
}

TEST_F(ComposedServiceDirTest, ServeFailsWithInvalidArgs) {
  // Serve should fail with an invalid channel.
  ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, {}), ZX_ERR_BAD_HANDLE);
  // Serve should fail with an invalid set of flags.
  {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(static_cast<fuchsia_io::Flags>(~0ull), std::move(root_server)),
              ZX_ERR_INVALID_ARGS);
  }
  // Non-directory protocols are disallowed.
  {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(fuchsia_io::Flags::kProtocolFile, std::move(root_server)),
              ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(ComposedServiceDirTest, ServeFailsWithDifferentDispatcher) {
  zx::channel root_client, root_server;
  // We should be able to serve the same node multiple times as long as we use the same dispatcher.
  for (size_t i = 0; i < 3; ++i) {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, std::move(root_server)), ZX_OK);
  }
  // Serve should fail with a different dispatcher. Only one dispatcher may be registered to serve
  // a given node.
  {
    async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, std::move(root_server), loop.dispatcher()),
              ZX_ERR_INVALID_ARGS);
  }
}

}  // namespace
