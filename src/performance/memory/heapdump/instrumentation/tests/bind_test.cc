// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.memory.heapdump.process/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/zx/channel.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/process.h>

#include <gtest/gtest.h>
#include <heapdump/bind.h>

// Why death tests?
//
// All the tests in this file call heapdump_bind_with_channel, which can only be called once per
// process. Calling it multiple times within the same process results in a panic, by design (as the
// "BindTwiceShouldCrash" test below verifies).
//
// Therefore, each test must run in isolation as a separate process. Since gtest runs death tests in
// subprocesses, expressing each test as a death test (ASSERT_EXIT or ASSERT_DEATH) is a convenient
// way to obtain this level of isolation.

/// Tests if a VMO with the given name is mapped in the current process.
static bool CheckVmoIsPresent(const char* name) {
  std::vector<zx_info_maps_t> mappings;

  size_t actual_count = 0;
  size_t avail_count = 0;
  do {
    zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, mappings.data(),
                                  mappings.size() * sizeof(zx_info_maps_t), &actual_count,
                                  &avail_count);
    mappings.resize(avail_count);
  } while (avail_count != actual_count);

  for (const zx_info_maps_t& entry : mappings) {
    puts(entry.name);
    if (entry.type == ZX_INFO_MAPS_TYPE_MAPPING && strcmp(entry.name, name) == 0) {
      return true;
    }
  }

  return false;
}

// Verify that binding to a registry given a channel results in the expected message being written.
TEST(BindTest, Success) {
  const auto action = [] {
    zx::channel client, server;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &client, &server));
    heapdump_bind_with_channel(client.release());

    class FakeRegistry : public fidl::WireServer<fuchsia_memory_heapdump_process::Registry> {
      void RegisterV1(fuchsia_memory_heapdump_process::wire::RegistryRegisterV1Request*,
                      RegisterV1Completer::Sync&) override {
        _exit(99);  // Test passed: exit with a known exit code.
      }
    };

    async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
    fidl::BindServer(loop.dispatcher(),
                     fidl::ServerEnd<fuchsia_memory_heapdump_process::Registry>(std::move(server)),
                     std::make_unique<FakeRegistry>());
    loop.Run();
  };

  // Run action() in a subprocess.
  ASSERT_EXIT(action(), testing::ExitedWithCode(99), "");
}

// Verify that passing an invalid handle tears down the instrumentation and does not make the
// program crash.
TEST(BindTest, InvalidHandle) {
  // This VMOs only exist while the instrumentation is active. They will no longer exist after the
  // instrumentation has been torn down as a result of passing an invalid handle.
  static const char kHeapdumpAllocationsVmoName[] = "heapdump-allocations";
  static const char kHeapdumpResourcesVmoName[] = "heapdump-resources";

  const auto action = [] {
    // Assert that the instrumentation is currently active.
    ASSERT_TRUE(CheckVmoIsPresent(kHeapdumpAllocationsVmoName));
    ASSERT_TRUE(CheckVmoIsPresent(kHeapdumpResourcesVmoName));

    heapdump_bind_with_channel(ZX_HANDLE_INVALID);

    // Assert that the instrumentation has been torn down.
    ASSERT_FALSE(CheckVmoIsPresent(kHeapdumpAllocationsVmoName));
    ASSERT_FALSE(CheckVmoIsPresent(kHeapdumpResourcesVmoName));

    _exit(99);  // Test passed: exit with a known exit code.
  };

  // Run action() in a subprocess.
  ASSERT_EXIT(action(), testing::ExitedWithCode(99), "");
}

// Verify that attempting to bind with a non-channel handle results in a crash.
TEST(BindTest, BadHandleTypeShouldCrash) {
  const auto action = [] {
    zx::eventpair ev1, ev2;
    ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &ev1, &ev2));
    heapdump_bind_with_channel(ev1.release());
  };

  // Run action() in a subprocess.
  ASSERT_DEATH(action(), "");
}

// Verify that attempting to bind twice results in a crash.
TEST(BindTest, BindTwiceShouldCrash) {
  const auto action = [] {
    heapdump_bind_with_channel(ZX_HANDLE_INVALID);
    heapdump_bind_with_channel(ZX_HANDLE_INVALID);
  };

  // Run action() in a subprocess and expect a panic.
  ASSERT_DEATH(action(), "panic");
}
