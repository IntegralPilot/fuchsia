// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/tests/test_driver.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/internal/driver_lifecycle.h>
#include <lib/driver/testing/cpp/internal/test_environment.h>
#include <lib/driver/testing/cpp/test_node.h>
#include <lib/fdf/env.h>
#include <lib/fdf/testing.h>

#include <gtest/gtest.h>

// WARNING!
// This test is using the old driver testing libraries. These were verbose and difficult to use
// directly. We have a new driver testing library that wraps the logic provided by these libraries
// and moves these into the internal namespace. The new library is in the header:
// <lib/driver/testing/cpp/driver_test.h>
// This library is the replacement that should be used from now on. It is also referred to
// as the driver test fixture library in some places due to historical reasons. See the tests over
// in fixture_based_tests.cc for example tests using the library.

// SEE WARNING AT TOP. DO NOT COPY INTO NEW TESTS.
// This is the recommended setup if you have a driver that doesn't need to make synchronous FIDL
// calls. Everything runs on the main test thread so everything is safe to access directly.
class TestForegroundDispatcher : public ::testing::Test {
 public:
  TestForegroundDispatcher()
      : device_server_(runtime_.StartBackgroundDispatcher()->async_dispatcher()) {}

  void SetUp() override {
    // Create start args
    node_server_.emplace("root");
    zx::result start_args = node_server_->CreateStartArgsAndServe();
    EXPECT_EQ(ZX_OK, start_args.status_value());

    // Start the test environment
    device_server_.emplace();
    device_server_.SyncCall([&](DeviceServer* device_server) {
      zx::result result =
          device_server->env.Initialize(std::move(start_args->incoming_directory_server));
      EXPECT_EQ(ZX_OK, result.status_value());

      device_server->server.Initialize(component::kDefaultInstance);
      EXPECT_EQ(ZX_OK,
                device_server->server.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                            &device_server->env.incoming_directory()));
    });

    // Start driver
    zx::result start_result =
        runtime_.RunToCompletion(driver_.Start(std::move(start_args->start_args)));
    EXPECT_EQ(ZX_OK, start_result.status_value());
  }

  void TearDown() override {
    zx::result prepare_stop_result = runtime_.RunToCompletion(driver_.PrepareStop());
    EXPECT_EQ(ZX_OK, prepare_stop_result.status_value());

    device_server_.reset();
    node_server_.reset();

    runtime_.ShutdownAllDispatchers(fdf::Dispatcher::GetCurrent()->get());
  }

  fdf_testing::internal::DriverUnderTest<TestDriver>& driver() { return driver_; }

 private:
  struct DeviceServer {
    fdf_testing::internal::TestEnvironment env;
    compat::DeviceServer server;
  };

  // Attaches a foreground dispatcher for us automatically.
  fdf_testing::DriverRuntime runtime_;

  // These will use the foreground dispatcher.
  std::optional<fdf_testing::TestNode> node_server_;
  async_patterns::TestDispatcherBound<DeviceServer> device_server_;
  fdf_testing::internal::DriverUnderTest<TestDriver> driver_;
};

TEST_F(TestForegroundDispatcher, CreateChildNodeAsync) {
  // Safe to touch driver since the dispatcher is in the foreground.
  // Dispatcher does not allow sync calls from the driver so we have to use the async version.
  bool initialized = false;
  driver()->BeginInitAsyncCompat([&initialized](zx::result<> result) {
    EXPECT_EQ(ZX_OK, result.status_value());
    initialized = true;
  });
  while (!initialized) {
    fdf_testing_run_until_idle();
  }

  driver()->CreateChildNodeAsync();
  while (!driver()->async_added_child()) {
    fdf_testing_run_until_idle();
  }
}

// SEE WARNING AT TOP. DO NOT COPY INTO NEW TESTS.
// If the environment needs to run on a background dispatcher (for example if the driver needs
// to make sync FIDL calls), we need to run the environment on a background dispatcher while keeping
// the driver on the main thread.
class TestForegroundDriverBackgroundEnv : public ::testing::Test {
 public:
  void SetUp() override {
    // Create start args
    zx::result start_args = node_server_.SyncCall(&fdf_testing::TestNode::CreateStartArgsAndServe);
    EXPECT_EQ(ZX_OK, start_args.status_value());

    // Start the test environment
    zx::result init_result =
        test_environment_.SyncCall(&fdf_testing::internal::TestEnvironment::Initialize,
                                   std::move(start_args->incoming_directory_server));
    EXPECT_EQ(ZX_OK, init_result.status_value());

    // test_environment_ and device_server live on the same dispatcher so moving the ptr from one
    // to the other is fine to do.
    fdf::OutgoingDirectory* outgoing_ptr;
    test_environment_.SyncCall([&outgoing_ptr](fdf_testing::internal::TestEnvironment* test_env) {
      outgoing_ptr = &test_env->incoming_directory();
    });
    device_server.SyncCall([outgoing_ptr](compat::DeviceServer* device_server) {
      device_server->Initialize(component::kDefaultInstance);
      EXPECT_EQ(ZX_OK, device_server->Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                            outgoing_ptr));
    });

    zx::result start_result =
        runtime_.RunToCompletion(driver_.Start(std::move(start_args->start_args)));
    EXPECT_EQ(ZX_OK, start_result.status_value());
  }

  void TearDown() override {
    zx::result stop_result = runtime_.RunToCompletion(driver_.PrepareStop());
    EXPECT_EQ(ZX_OK, stop_result.status_value());

    device_server.reset();
    test_environment_.reset();
    node_server_.reset();

    runtime_.ShutdownAllDispatchers(fdf::Dispatcher::GetCurrent()->get());
  }

  fdf_testing::internal::DriverUnderTest<TestDriver>& driver() { return driver_; }

  async_dispatcher_t* env_dispatcher() { return env_dispatcher_->async_dispatcher(); }

 private:
  // Attaches a foreground dispatcher for us automatically.
  fdf_testing::DriverRuntime runtime_;

  // Env dispatcher runs in the background because we need to make sync calls into it.
  fdf::UnownedSynchronizedDispatcher env_dispatcher_ = runtime_.StartBackgroundDispatcher();

  async_patterns::TestDispatcherBound<fdf_testing::TestNode> node_server_{
      env_dispatcher(), std::in_place, std::string("root")};

  async_patterns::TestDispatcherBound<fdf_testing::internal::TestEnvironment> test_environment_{
      env_dispatcher(), std::in_place};

  async_patterns::TestDispatcherBound<compat::DeviceServer> device_server{env_dispatcher(),
                                                                          std::in_place};

  fdf_testing::internal::DriverUnderTest<TestDriver> driver_;
};

TEST_F(TestForegroundDriverBackgroundEnv, CreateChildNodeSync) {
  // Safe to touch the driver from here since the dispatcher is in the foreground.
  // Dispatcher allows sync calls from the driver so we use the sync version.
  EXPECT_EQ(ZX_OK, driver()->InitSyncCompat().status_value());
  driver()->CreateChildNodeSync();
  EXPECT_TRUE(driver()->sync_added_child());
}
