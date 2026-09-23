// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/directory.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

namespace provider = fuchsia_tracing_provider;

namespace {
const char kProviderRegistryPath[] = "svc/fuchsia.tracing.provider.Registry";
}  // namespace

// Test registration using the V2 protocol.
TEST_F(TraceManagerTest, RegisterV2Provider) {
  zx::channel h1, h2;
  ASSERT_EQ(zx::channel::create(0, &h1, &h2), ZX_OK);
  ASSERT_EQ(fdio_service_connect_at(context_provider().outgoing_directory_ptr().channel().get(),
                                    kProviderRegistryPath, h2.release()),
            ZX_OK);
  fidl::Client<provider::Registry> registry_ptr{fidl::ClientEnd<provider::Registry>{std::move(h1)},
                                                dispatcher()};

  zx::channel ph1a, ph1b;
  ASSERT_EQ(zx::channel::create(0, &ph1a, &ph1b), ZX_OK);
  fidl::ClientEnd<provider::ProviderV2> provider1_h{std::move(ph1b)};
  auto result1 =
      registry_ptr->RegisterV2({{std::move(provider1_h), kProvider1Pid, kProvider1Name}});
  ASSERT_TRUE(result1.is_ok());

  RunLoopUntilIdle();

  ConnectToProvisionerService();
  std::vector<fuchsia_tracing_controller::ProviderInfo> providers;
  provisioner_client()->GetProviders().ThenExactlyOnce([&providers](auto& result) {
    ASSERT_TRUE(result.is_ok());
    providers = std::move(result.value().providers());
  });
  RunLoopUntilIdle();

  EXPECT_EQ(providers.size(), 1u);
  if (!providers.empty()) {
    EXPECT_EQ(providers[0].pid().value_or(0), kProvider1Pid);
    EXPECT_STREQ(providers[0].name().value_or("").c_str(), kProvider1Name);
  }
}

// Test synchronous registration using the V2 protocol.
TEST_F(TraceManagerTest, RegisterV2ProviderSynchronously) {
  zx::channel h1, h2;
  ASSERT_EQ(zx::channel::create(0, &h1, &h2), ZX_OK);
  ASSERT_EQ(fdio_service_connect_at(context_provider().outgoing_directory_ptr().channel().get(),
                                    kProviderRegistryPath, h2.release()),
            ZX_OK);
  fidl::Client<provider::Registry> registry_ptr{fidl::ClientEnd<provider::Registry>{std::move(h1)},
                                                dispatcher()};

  zx::channel ph1a, ph1b;
  ASSERT_EQ(zx::channel::create(0, &ph1a, &ph1b), ZX_OK);
  fidl::ClientEnd<provider::ProviderV2> provider1_h{std::move(ph1b)};

  bool register_completed = false;
  registry_ptr->RegisterV2Synchronously({{std::move(provider1_h), kProvider1Pid, kProvider1Name}})
      .ThenExactlyOnce(
          [&register_completed](
              fidl::Result<fuchsia_tracing_provider::Registry::RegisterV2Synchronously>& result) {
            ASSERT_TRUE(result.is_ok());
            EXPECT_EQ(result.value().started(), ZX_OK);
            register_completed = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(register_completed);

  ConnectToProvisionerService();
  std::vector<fuchsia_tracing_controller::ProviderInfo> providers;
  provisioner_client()->GetProviders().ThenExactlyOnce([&providers](auto& result) {
    ASSERT_TRUE(result.is_ok());
    providers = std::move(result.value().providers());
  });
  RunLoopUntilIdle();

  EXPECT_EQ(providers.size(), 1u);
}

// Test full lifecycle of a V2 provider.
TEST_F(TraceManagerTest, V2ProviderLifecycle) {
  ConnectToProvisionerService();

  FakeProviderV2* provider1;
  ASSERT_TRUE(AddFakeProviderV2(kProvider1Pid, kProvider1Name, &provider1));

  RunLoopUntilIdle();

  // Initialize session.
  ASSERT_TRUE(InitializeSession());
  EXPECT_EQ(provider1->state(), FakeProviderV2::State::kInitialized);
  EXPECT_EQ(provider1->initialize_count(), 1);

  // Start tracing.
  BeginStartSession();
  ASSERT_TRUE(FinishStartSession());
  EXPECT_EQ(provider1->state(), FakeProviderV2::State::kStarted);
  EXPECT_EQ(provider1->start_count(), 1);

  // Stop tracing.
  BeginStopSession();
  ASSERT_TRUE(FinishStopSession());
  EXPECT_EQ(provider1->state(), FakeProviderV2::State::kStopped);
  EXPECT_EQ(provider1->stop_count(), 1);

  // Terminate session.
  BeginTerminateSession();
  ASSERT_TRUE(FinishTerminateSession());
  EXPECT_EQ(provider1->state(), FakeProviderV2::State::kTerminated);
  EXPECT_EQ(provider1->terminate_count(), 1);
}

}  // namespace test
}  // namespace tracing
