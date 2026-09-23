// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.component.test/cpp/driver/wire.h>
#include <fidl/fuchsia.driver.component.test/cpp/wire.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/driver/component/cpp/tests/test_driver.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/internal/driver_lifecycle.h>
#include <lib/driver/testing/cpp/internal/test_environment.h>
#include <lib/driver/testing/cpp/test_node.h>
#include <lib/fdio/directory.h>

#include <gtest/gtest.h>

// WARNING!
// This test is using the old driver testing libraries. These were verbose and difficult to use
// directly. We have a new driver testing library that wraps the logic provided by these libraries
// and moves these into the internal namespace. The new library is in the header:
// <lib/driver/testing/cpp/driver_test.h>
// This library is the replacement that should be used from now on. It is also referred to
// as the driver test fixture library in some places due to historical reasons. See the tests over
// in fixture_based_tests.cc for example tests using the library.

class ZirconProtocolServer
    : public fidl::WireServer<fuchsia_driver_component_test::ZirconProtocol> {
 public:
  fuchsia_driver_component_test::ZirconService::InstanceHandler GetInstanceHandler() {
    return fuchsia_driver_component_test::ZirconService::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

 private:
  // fidl::WireServer<fuchsia_driver_component_test::ZirconProtocol>
  void ZirconMethod(ZirconMethodCompleter::Sync& completer) override { completer.ReplySuccess(); }
  fidl::ServerBindingGroup<fuchsia_driver_component_test::ZirconProtocol> bindings_;
};

// [START provide_handler]
class DriverProtocolServer : public fdf::WireServer<fuchsia_driver_component_test::DriverProtocol> {
 public:
  fuchsia_driver_component_test::DriverService::InstanceHandler GetInstanceHandler() {
    return fuchsia_driver_component_test::DriverService::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

 private:
  // fdf::WireServer<fuchsia_driver_component_test::DriverProtocol>
  void DriverMethod(fdf::Arena& arena, DriverMethodCompleter::Sync& completer) override {
    fdf::Arena reply_arena('TEST');
    completer.buffer(reply_arena).ReplySuccess();
  }

  fdf::ServerBindingGroup<fuchsia_driver_component_test::DriverProtocol> bindings_;
};
// [END provide_handler]

// SEE WARNING AT TOP. DO NOT COPY INTO NEW TESTS.
// Sets up the environment to have both Zircon and Driver transport services.
class TestIncomingAndOutgoingFidlsBase : public ::testing::Test {
 public:
  void SetUp() override {
    // Create start args
    zx::result start_args = node_server_.SyncCall(&fdf_testing::TestNode::CreateStartArgsAndServe);
    ASSERT_EQ(ZX_OK, start_args.status_value());

    driver_outgoing_ = std::move(start_args->outgoing_directory_client);

    // Start the test environment
    // [START initialize_test_environment]
    zx::result init_result =
        test_environment_.SyncCall(&fdf_testing::internal::TestEnvironment::Initialize,
                                   std::move(start_args->incoming_directory_server));
    ASSERT_EQ(ZX_OK, init_result.status_value());
    // [END initialize_test_environment]

    // Add the services to the environment.
    // [START get_server_handlers]
    fuchsia_driver_component_test::ZirconService::InstanceHandler zircon_proto_handler =
        zircon_proto_server_.SyncCall(&ZirconProtocolServer::GetInstanceHandler);
    fuchsia_driver_component_test::DriverService::InstanceHandler driver_proto_handler =
        driver_proto_server_.SyncCall(&DriverProtocolServer::GetInstanceHandler);
    // [END get_server_handlers]

    // [START move_server_handlers]
    test_environment_.SyncCall([zircon_proto_handler = std::move(zircon_proto_handler),
                                driver_proto_handler = std::move(driver_proto_handler)](
                                   fdf_testing::internal::TestEnvironment* env) mutable {
      zx::result result =
          env->incoming_directory().AddService<fuchsia_driver_component_test::ZirconService>(
              std::move(zircon_proto_handler));
      ASSERT_EQ(ZX_OK, result.status_value());

      result = env->incoming_directory().AddService<fuchsia_driver_component_test::DriverService>(
          std::move(driver_proto_handler));
      ASSERT_EQ(ZX_OK, result.status_value());
    });
    // [END move_server_handlers]

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

    // Store the start_args for the subclasses to use to start the driver.
    start_args_ = std::move(start_args->start_args);
  }

  void TearDown() override {
    device_server.reset();
    test_environment_.reset();
    node_server_.reset();
    driver_proto_server_.reset();
    zircon_proto_server_.reset();
  }

  async_patterns::TestDispatcherBound<fdf_testing::TestNode>& node_server() { return node_server_; }

  fidl::ClientEnd<fuchsia_io::Directory> CreateDriverSvcClient() {
    // Open the svc directory in the driver's outgoing, and store a client to it.
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();

    // [START set_up_outgoing_directory_channel]
    zx_status_t status = fdio_open3_at(
        driver_outgoing_.handle()->get(), "/svc",
        uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::Flags::kProtocolDirectory},
        server_end.TakeChannel().release());
    // [END set_up_outgoing_directory_channel]
    EXPECT_EQ(ZX_OK, status);
    return std::move(client_end);
  }

  async_dispatcher_t* env_dispatcher() { return env_dispatcher_->async_dispatcher(); }

 protected:
  fuchsia_driver_framework::DriverStartArgs& start_args() { return start_args_; }
  fdf_testing::DriverRuntime& runtime() { return runtime_; }

 private:
  // Attaches a foreground dispatcher for us automatically.
  fdf_testing::DriverRuntime runtime_;

  // Env dispatcher. Managed by driver runtime threads.
  fdf::UnownedSynchronizedDispatcher env_dispatcher_ = runtime_.StartBackgroundDispatcher();

  // Servers for the incoming FIDLs to the driver.
  // [START custom_server_classes]
  async_patterns::TestDispatcherBound<ZirconProtocolServer> zircon_proto_server_{env_dispatcher(),
                                                                                 std::in_place};
  async_patterns::TestDispatcherBound<DriverProtocolServer> driver_proto_server_{env_dispatcher(),
                                                                                 std::in_place};
  // [END custom_server_classes]

  async_patterns::TestDispatcherBound<compat::DeviceServer> device_server{env_dispatcher(),
                                                                          std::in_place};

  // Serves the fdf::Node protocol to the driver.
  async_patterns::TestDispatcherBound<fdf_testing::TestNode> node_server_{
      env_dispatcher(), std::in_place, std::string("root")};

  // The environment can serve both the Zircon and Driver transport based protocols to the driver.
  async_patterns::TestDispatcherBound<fdf_testing::internal::TestEnvironment> test_environment_{
      env_dispatcher(), std::in_place};

  fidl::ClientEnd<fuchsia_io::Directory> driver_outgoing_;

  fuchsia_driver_framework::DriverStartArgs start_args_;
};

// SEE WARNING AT TOP. DO NOT COPY INTO NEW TESTS.
// Set the driver dispatcher to default so we can access |driver()| directly.
class TestIncomingAndOutgoingFidlsDefaultDriver : public TestIncomingAndOutgoingFidlsBase {
 public:
  // Sync clients into the driver have to be ran on a background thread because the test thread is
  // where the driver will handle the call.
  static void RunSyncClientTask(fit::closure task) {
    // Spawn a separate thread to run the client task using an async::Loop.
    async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
    loop.StartThread();
    zx::result result = fdf::RunOnDispatcherSync(loop.dispatcher(), std::move(task));
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void SetUp() override {
    TestIncomingAndOutgoingFidlsBase::SetUp();
    zx::result result = runtime().RunToCompletion(driver_.Start(std::move(start_args())));
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    zx::result result = runtime().RunToCompletion(driver_.PrepareStop());
    ASSERT_EQ(ZX_OK, result.status_value());

    // Tear down the environment after the driver goes through PrepareStop.
    TestIncomingAndOutgoingFidlsBase::TearDown();

    runtime().ShutdownAllDispatchers(fdf::Dispatcher::GetCurrent()->get());
  }

  TestDriver* driver() { return *driver_; }

 private:
  // The driver under test.
  fdf_testing::internal::DriverUnderTest<TestDriver> driver_;
};

TEST_F(TestIncomingAndOutgoingFidlsDefaultDriver, ValidateDriverIncomingServices) {
  zx::result result = driver()->ValidateIncomingDriverService();
  ASSERT_EQ(ZX_OK, result.status_value());
  result = driver()->ValidateIncomingZirconService();
  ASSERT_EQ(ZX_OK, result.status_value());
}

TEST_F(TestIncomingAndOutgoingFidlsDefaultDriver, ConnectWithDevfs) {
  zx::result export_result = driver()->ExportDevfsNodeSync();
  ASSERT_EQ(ZX_OK, export_result.status_value());

  zx::result device_result = node_server().SyncCall([](fdf_testing::TestNode* root_node) {
    return root_node->children().at("devfs_node").ConnectToDevice();
  });

  ASSERT_EQ(ZX_OK, device_result.status_value());
  fidl::ClientEnd<fuchsia_driver_component_test::ZirconProtocol> device_client_end(
      std::move(device_result.value()));
  fidl::WireSyncClient<fuchsia_driver_component_test::ZirconProtocol> zircon_proto_client(
      std::move(device_client_end));

  RunSyncClientTask([zircon_proto_client = std::move(zircon_proto_client)]() {
    fidl::WireResult result = zircon_proto_client->ZirconMethod();
    ASSERT_EQ(ZX_OK, result.status());
    ASSERT_EQ(true, result.value().is_ok());
  });
}

TEST_F(TestIncomingAndOutgoingFidlsDefaultDriver, ConnectWithZirconService) {
  zx::result serve_result = driver()->ServeZirconService();
  ASSERT_EQ(ZX_OK, serve_result.status_value());

  zx::result result =
      component::ConnectAtMember<fuchsia_driver_component_test::ZirconService::Device>(
          CreateDriverSvcClient());
  ASSERT_EQ(ZX_OK, result.status_value());

  RunSyncClientTask([client_end = std::move(result.value())]() {
    fidl::WireResult wire_result = fidl::WireCall(client_end)->ZirconMethod();
    ASSERT_EQ(ZX_OK, wire_result.status());
    ASSERT_EQ(true, wire_result.value().is_ok());
  });
}

TEST_F(TestIncomingAndOutgoingFidlsDefaultDriver, ConnectWithDriverService) {
  zx::result serve_result = driver()->ServeDriverService();
  ASSERT_EQ(ZX_OK, serve_result.status_value());

  zx::result driver_connect_result =
      fdf::internal::DriverTransportConnect<fuchsia_driver_component_test::DriverService::Device>(
          CreateDriverSvcClient(), component::kDefaultInstance);
  ASSERT_EQ(ZX_OK, driver_connect_result.status_value());

  RunSyncClientTask([client_end = std::move(driver_connect_result.value())]() {
    fdf::Arena arena('TEST');
    fdf::WireUnownedResult wire_result = fdf::WireCall(client_end).buffer(arena)->DriverMethod();
    ASSERT_EQ(ZX_OK, wire_result.status());
    ASSERT_EQ(true, wire_result.value().is_ok());
  });
}

// SEE WARNING AT TOP. DO NOT COPY INTO NEW TESTS.
// Set the driver dispatcher to be managed so that we can make sync client calls into the driver
// hosted services directly from the test instead of using |RunSyncClientTask| from above.
class TestIncomingAndOutgoingFidlsManagedDriver : public TestIncomingAndOutgoingFidlsBase {
 public:
  void SetUp() override {
    TestIncomingAndOutgoingFidlsBase::SetUp();
    zx::result result = runtime().RunToCompletion(driver_.SyncCall(
        &fdf_testing::internal::DriverUnderTest<TestDriver>::Start, std::move(start_args())));
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    zx::result result = runtime().RunToCompletion(
        driver_.SyncCall(&fdf_testing::internal::DriverUnderTest<TestDriver>::PrepareStop));
    ASSERT_EQ(ZX_OK, result.status_value());

    // Tear down the environment after the driver goes through PrepareStop.
    TestIncomingAndOutgoingFidlsBase::TearDown();

    runtime().ShutdownAllDispatchers(driver_dispatcher_->get());
  }

  async_patterns::TestDispatcherBound<fdf_testing::internal::DriverUnderTest<TestDriver>>&
  driver() {
    return driver_;
  }
  async_dispatcher_t* driver_dispatcher() { return driver_dispatcher_->async_dispatcher(); }

 private:
  // Driver dispatcher set as a background dispatcher.
  fdf::UnownedSynchronizedDispatcher driver_dispatcher_ = runtime().StartBackgroundDispatcher();

  // The driver under test.
  async_patterns::TestDispatcherBound<fdf_testing::internal::DriverUnderTest<TestDriver>> driver_{
      driver_dispatcher(), std::in_place};
};

TEST_F(TestIncomingAndOutgoingFidlsManagedDriver, ConnectWithDevfs) {
  driver().SyncCall([](fdf_testing::internal::DriverUnderTest<TestDriver>* driver) {
    zx::result result = (*driver)->ExportDevfsNodeSync();
    ASSERT_EQ(ZX_OK, result.status_value());
  });

  zx::result device_result = node_server().SyncCall([](fdf_testing::TestNode* root_node) {
    return root_node->children().at("devfs_node").ConnectToDevice();
  });

  ASSERT_EQ(ZX_OK, device_result.status_value());
  fidl::ClientEnd<fuchsia_driver_component_test::ZirconProtocol> device_client_end(
      std::move(device_result.value()));
  fidl::WireSyncClient<fuchsia_driver_component_test::ZirconProtocol> zircon_proto_client(
      std::move(device_client_end));

  fidl::WireResult result = zircon_proto_client->ZirconMethod();
  ASSERT_EQ(ZX_OK, result.status());
  ASSERT_EQ(true, result.value().is_ok());
}

TEST_F(TestIncomingAndOutgoingFidlsManagedDriver, ConnectWithZirconService) {
  driver().SyncCall([](fdf_testing::internal::DriverUnderTest<TestDriver>* driver) {
    zx::result result = (*driver)->ServeZirconService();
    ASSERT_EQ(ZX_OK, result.status_value());
  });

  zx::result result =
      component::ConnectAtMember<fuchsia_driver_component_test::ZirconService::Device>(
          CreateDriverSvcClient());
  ASSERT_EQ(ZX_OK, result.status_value());

  fidl::WireResult wire_result = fidl::WireCall(result.value())->ZirconMethod();
  ASSERT_EQ(ZX_OK, wire_result.status());
  ASSERT_EQ(true, wire_result.value().is_ok());
}

TEST_F(TestIncomingAndOutgoingFidlsManagedDriver, ConnectWithDriverService) {
  driver().SyncCall([](fdf_testing::internal::DriverUnderTest<TestDriver>* driver) {
    zx::result result = (*driver)->ServeDriverService();
    ASSERT_EQ(ZX_OK, result.status_value());
  });

  zx::result driver_connect_result =
      fdf::internal::DriverTransportConnect<fuchsia_driver_component_test::DriverService::Device>(
          CreateDriverSvcClient(), component::kDefaultInstance);
  ASSERT_EQ(ZX_OK, driver_connect_result.status_value());

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult wire_result =
      fdf::WireCall(driver_connect_result.value()).buffer(arena)->DriverMethod();
  ASSERT_EQ(ZX_OK, wire_result.status());
  ASSERT_EQ(true, wire_result.value().is_ok());
}
