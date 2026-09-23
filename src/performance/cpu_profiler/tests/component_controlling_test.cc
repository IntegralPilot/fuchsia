// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.cpu.profiler/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/result.h>

#include <vector>

#include <gtest/gtest.h>

// If we fail to launch a component, ensure that it gets cleaned up properly.
TEST(ComponentControlling, CleanUpFailedLaunch) {
  zx::result client_end = component::Connect<fuchsia_cpu_profiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  fuchsia_cpu_profiler::SamplingConfig sampling_config{{
      .period = 1000000,
      .timebase = fuchsia_cpu_profiler::Counter::WithPlatformIndependent(
          fuchsia_cpu_profiler::CounterId::kNanoseconds),
      .sample = fuchsia_cpu_profiler::Sample{{
          .callgraph =
              fuchsia_cpu_profiler::CallgraphConfig{
                  {.strategy = fuchsia_cpu_profiler::CallgraphStrategy::kFramePointer}},
          .counters = {},
      }},
  }};

  // Attempt to launch a component that will be created, but will fail to resolve.
  fuchsia_cpu_profiler::TargetConfig target_config =
      fuchsia_cpu_profiler::TargetConfig::WithComponent(
          fuchsia_cpu_profiler::AttachConfig::WithLaunchComponent({{
              .url = "not_found#meta/not_found.cm",
              .moniker = "./launchpad:not_found",
          }}));

  ASSERT_TRUE(client
                  ->Configure({{.output = std::move(outgoing_socket),
                                .config = fuchsia_cpu_profiler::Config{{
                                    .configs = std::vector{sampling_config},
                                    .target = std::move(target_config),
                                }}}})
                  .is_error());

  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());
  fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  auto res = lifecycle_client->DestroyInstance({{.parent_moniker = ".",
                                                 .child = {{
                                                     .name = "not_found",
                                                     .collection = "launchpad",
                                                 }}}});
  ASSERT_TRUE(res.is_error());
  ASSERT_TRUE(res.error_value().is_domain_error());
  ASSERT_EQ(res.error_value().domain_error(), fuchsia_sys2::DestroyError::kInstanceNotFound);
}
