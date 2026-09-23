// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/namespace/namespace-dfv1.h"

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/test.display.namespace/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

class MultiProtocolService : public fidl::Server<test_display_namespace::Incrementer>,
                             public fidl::Server<test_display_namespace::Decrementer> {
 public:
  MultiProtocolService() = default;
  ~MultiProtocolService() = default;

  // implements [`test.display.namespace/Incrementer`].
  void Increment(IncrementRequest& request, IncrementCompleter::Sync& completer) override {
    ZX_DEBUG_ASSERT(request.x() >= -32768);
    ZX_DEBUG_ASSERT(request.x() <= 32768);
    completer.Reply({{.result = request.x() + 1}});
  }

  // implements [`test.display.namespace/Decrementer`].
  void Decrement(DecrementRequest& request, DecrementCompleter::Sync& completer) override {
    ZX_DEBUG_ASSERT(request.x() >= -32768);
    ZX_DEBUG_ASSERT(request.x() <= 32768);
    completer.Reply({{.result = request.x() - 1}});
  }

  test_display_namespace::MultiProtocolService::InstanceHandler GetServiceInstanceHandler(
      async_dispatcher_t* dispatcher) {
    return test_display_namespace::MultiProtocolService::InstanceHandler({
        .incrementer =
            incrementer_bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
        .decrementer =
            decrementer_bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });
  }

 private:
  fidl::ServerBindingGroup<test_display_namespace::Incrementer> incrementer_bindings_;
  fidl::ServerBindingGroup<test_display_namespace::Decrementer> decrementer_bindings_;
};

class SingleProtocolService : public fidl::Server<test_display_namespace::Echo> {
 public:
  SingleProtocolService() = default;

  // implements [`test.display.namespace/Echo`].
  void EchoInt32(EchoInt32Request& request, EchoInt32Completer::Sync& completer) override {
    completer.Reply({{.result = request.x()}});
  }

  test_display_namespace::SingleProtocolService::InstanceHandler GetServiceInstanceHandler(
      async_dispatcher_t* dispatcher) {
    return test_display_namespace::SingleProtocolService::InstanceHandler({
        .echo = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });
  }

 private:
  fidl::ServerBindingGroup<test_display_namespace::Echo> bindings_;
};

class NamespaceDfv1Test : public testing::Test {
 public:
  // implements `testing::Test`.
  void SetUp() override {
    incoming_loop_.StartThread("incoming-loop-thread");
    fake_parent_ = MockDevice::FakeRootParent();

    {
      zx::result<fidl::Endpoints<fuchsia_io::Directory>> multi_protocol_directory_result =
          fidl::CreateEndpoints<fuchsia_io::Directory>();
      ASSERT_OK(multi_protocol_directory_result.status_value());
      auto [multi_protocol_directory_client, multi_protocol_directory_server] =
          std::move(multi_protocol_directory_result).value();

      multi_protocol_outgoing_.SyncCall(
          [this, multi_protocol_directory_server = std::move(multi_protocol_directory_server)](
              component::OutgoingDirectory* outgoing) mutable {
            zx::result<> multi_protocol_add_service_result =
                outgoing->AddService<test_display_namespace::MultiProtocolService>(
                    multi_protocol_service_.GetServiceInstanceHandler(incoming_loop_.dispatcher()));
            ASSERT_OK(multi_protocol_add_service_result.status_value());

            zx::result<> multi_protocol_serve_result =
                outgoing->Serve(std::move(multi_protocol_directory_server));
            ASSERT_OK(multi_protocol_serve_result.status_value());
          });

      fake_parent_->AddFidlService(test_display_namespace::MultiProtocolService::Name,
                                   std::move(multi_protocol_directory_client),
                                   /*name=*/"multi-protocol-fragment");
    }

    {
      zx::result<fidl::Endpoints<fuchsia_io::Directory>> single_protocol_directory_result =
          fidl::CreateEndpoints<fuchsia_io::Directory>();
      ASSERT_OK(single_protocol_directory_result.status_value());
      auto [single_protocol_directory_client, single_protocol_directory_server] =
          std::move(single_protocol_directory_result).value();

      single_protocol_outgoing_.SyncCall([this, single_protocol_directory_server =
                                                    std::move(single_protocol_directory_server)](
                                             component::OutgoingDirectory* outgoing) mutable {
        zx::result<> single_protocol_add_service_result =
            outgoing->AddService<test_display_namespace::SingleProtocolService>(
                single_protocol_service_.GetServiceInstanceHandler(incoming_loop_.dispatcher()));
        ASSERT_OK(single_protocol_add_service_result.status_value());

        zx::result<> single_protocol_serve_result =
            outgoing->Serve(std::move(single_protocol_directory_server));
        ASSERT_OK(single_protocol_serve_result.status_value());
      });

      fake_parent_->AddFidlService(test_display_namespace::SingleProtocolService::Name,
                                   std::move(single_protocol_directory_client),
                                   /*name=*/"single-protocol-fragment");
    }
  }

  // implements `testing::Test`.
  void TearDown() override {
    multi_protocol_outgoing_.reset();
    single_protocol_outgoing_.reset();
    incoming_loop_.Shutdown();
  }

  MockDevice* fake_parent() const { return fake_parent_.get(); }

 private:
  std::shared_ptr<MockDevice> fake_parent_;

  async::Loop incoming_loop_{&kAsyncLoopConfigNeverAttachToThread};

  MultiProtocolService multi_protocol_service_;
  SingleProtocolService single_protocol_service_;

  async_patterns::TestDispatcherBound<component::OutgoingDirectory> multi_protocol_outgoing_{
      incoming_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
  async_patterns::TestDispatcherBound<component::OutgoingDirectory> single_protocol_outgoing_{
      incoming_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
};

TEST_F(NamespaceDfv1Test, ConnectToFidlProtocol) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::ClientEnd<test_display_namespace::Incrementer>> connect_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          "multi-protocol-fragment");
  ASSERT_OK(connect_result.status_value());

  fidl::SyncClient incrementer_client(std::move(connect_result).value());
  fidl::Result increment_result = incrementer_client->Increment({{.x = 1}});
  ASSERT_TRUE(increment_result.is_ok());

  EXPECT_EQ(increment_result.value().result(), 2);
}

TEST_F(NamespaceDfv1Test, ConnectToFidlProtocolUsingProvidedServerEnd) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Incrementer>> incrementer_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Incrementer>();
  ASSERT_OK(incrementer_endpoints_result.status_value());
  auto [incrementer_client_end, incrementer_server_end] =
      std::move(incrementer_endpoints_result).value();

  zx::result<> connect_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          std::move(incrementer_server_end), "multi-protocol-fragment");
  ASSERT_OK(connect_result.status_value());

  fidl::SyncClient incrementer_client(std::move(incrementer_client_end));
  fidl::Result increment_result = incrementer_client->Increment({{.x = 1}});
  ASSERT_TRUE(increment_result.is_ok());

  EXPECT_EQ(increment_result.value().result(), 2);
}

TEST_F(NamespaceDfv1Test, ConnectToDifferentServiceMemberProtocols) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::ClientEnd<test_display_namespace::Incrementer>> connect_incrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          "multi-protocol-fragment");
  ASSERT_OK(connect_incrementer_result.status_value());

  zx::result<fidl::ClientEnd<test_display_namespace::Decrementer>> connect_decrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Decrementer>(
          "multi-protocol-fragment");
  ASSERT_OK(connect_decrementer_result.status_value());

  fidl::SyncClient incrementer_client(std::move(connect_incrementer_result).value());
  fidl::Result increment_result = incrementer_client->Increment({{.x = 1}});
  ASSERT_TRUE(increment_result.is_ok());
  EXPECT_EQ(increment_result.value().result(), 2);

  fidl::SyncClient decrementer_client(std::move(connect_decrementer_result).value());
  fidl::Result decrement_result = decrementer_client->Decrement({{.x = 1}});
  ASSERT_TRUE(decrement_result.is_ok());
  EXPECT_EQ(decrement_result.value().result(), 0);
}

TEST_F(NamespaceDfv1Test, ConnectToDifferentServiceMemberProtocolsUsingProvidedServerEnd) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Incrementer>> incrementer_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Incrementer>();
  ASSERT_OK(incrementer_endpoints_result.status_value());
  auto [incrementer_client_end, incrementer_server_end] =
      std::move(incrementer_endpoints_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Decrementer>> decrementer_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Decrementer>();
  ASSERT_OK(decrementer_endpoints_result.status_value());
  auto [decrementer_client_end, decrementer_server_end] =
      std::move(decrementer_endpoints_result).value();

  zx::result<> connect_incrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          std::move(incrementer_server_end), "multi-protocol-fragment");
  ASSERT_OK(connect_incrementer_result.status_value());

  zx::result<> connect_decrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Decrementer>(
          std::move(decrementer_server_end), "multi-protocol-fragment");
  ASSERT_OK(connect_decrementer_result.status_value());

  fidl::SyncClient incrementer_client(std::move(incrementer_client_end));
  fidl::Result increment_result = incrementer_client->Increment({{.x = 1}});
  ASSERT_TRUE(increment_result.is_ok());
  EXPECT_EQ(increment_result.value().result(), 2);

  fidl::SyncClient decrementer_client(std::move(decrementer_client_end));
  fidl::Result decrement_result = decrementer_client->Decrement({{.x = 1}});
  ASSERT_TRUE(decrement_result.is_ok());
  EXPECT_EQ(decrement_result.value().result(), 0);
}

TEST_F(NamespaceDfv1Test, ConnectToDifferentFragments) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::ClientEnd<test_display_namespace::Incrementer>> connect_incrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          "multi-protocol-fragment");
  ASSERT_OK(connect_incrementer_result.status_value());

  zx::result<fidl::ClientEnd<test_display_namespace::Echo>> connect_echo_result =
      incoming->Connect<test_display_namespace::SingleProtocolService::Echo>(
          "single-protocol-fragment");
  ASSERT_OK(connect_echo_result.status_value());

  fidl::SyncClient incrementer_client(std::move(connect_incrementer_result).value());
  fidl::Result increment_result = incrementer_client->Increment({{.x = 1}});
  ASSERT_TRUE(increment_result.is_ok());
  EXPECT_EQ(increment_result.value().result(), 2);

  fidl::SyncClient echo_client(std::move(connect_echo_result).value());
  fidl::Result echo_result = echo_client->EchoInt32({{.x = 1}});
  ASSERT_TRUE(echo_result.is_ok());
  EXPECT_EQ(echo_result.value().result(), 1);
}

TEST_F(NamespaceDfv1Test, ConnectToDifferentFragmentsUsingProvidedServerEnd) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Incrementer>> incrementer_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Incrementer>();
  ASSERT_OK(incrementer_endpoints_result.status_value());
  auto [incrementer_client_end, incrementer_server_end] =
      std::move(incrementer_endpoints_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Echo>> echo_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Echo>();
  ASSERT_OK(echo_endpoints_result.status_value());
  auto [echo_client_end, echo_server_end] = std::move(echo_endpoints_result).value();

  zx::result<> connect_incrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          std::move(incrementer_server_end), "multi-protocol-fragment");
  ASSERT_OK(connect_incrementer_result.status_value());

  zx::result<> connect_echo_result =
      incoming->Connect<test_display_namespace::SingleProtocolService::Echo>(
          std::move(echo_server_end), "single-protocol-fragment");
  ASSERT_OK(connect_echo_result.status_value());

  fidl::SyncClient incrementer_client(std::move(incrementer_client_end));
  fidl::Result increment_result = incrementer_client->Increment({{.x = 1}});
  ASSERT_TRUE(increment_result.is_ok());
  EXPECT_EQ(increment_result.value().result(), 2);

  fidl::SyncClient echo_client(std::move(echo_client_end));
  fidl::Result echo_result = echo_client->EchoInt32({{.x = 1}});
  ASSERT_TRUE(echo_result.is_ok());
  EXPECT_EQ(echo_result.value().result(), 1);
}

TEST_F(NamespaceDfv1Test, ErrorOnInvalidFragment) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Incrementer>> incrementer_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Incrementer>();
  ASSERT_OK(incrementer_endpoints_result.status_value());
  auto [incrementer_client_end, incrementer_server_end] =
      std::move(incrementer_endpoints_result).value();

  zx::result<fidl::ClientEnd<test_display_namespace::Incrementer>> connect_incrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          "invalid-fragment");
  ASSERT_NE(connect_incrementer_result.status_value(), ZX_OK);

  zx::result<> connect_incrementer_using_server_end_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          std::move(incrementer_server_end), "invalid-fragment");
  ASSERT_NE(connect_incrementer_using_server_end_result.status_value(), ZX_OK);
}

TEST_F(NamespaceDfv1Test, ErrorOnServiceDoesntMatchFragment) {
  zx::result<std::unique_ptr<Namespace>> incoming_result = NamespaceDfv1::Create(fake_parent());
  ASSERT_OK(incoming_result.status_value());
  std::unique_ptr<Namespace> incoming = std::move(incoming_result).value();

  zx::result<fidl::Endpoints<test_display_namespace::Incrementer>> incrementer_endpoints_result =
      fidl::CreateEndpoints<test_display_namespace::Incrementer>();
  ASSERT_OK(incrementer_endpoints_result.status_value());
  auto [incrementer_client_end, incrementer_server_end] =
      std::move(incrementer_endpoints_result).value();

  zx::result<fidl::ClientEnd<test_display_namespace::Incrementer>> connect_incrementer_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          "single-protocol-fragment");
  ASSERT_NE(connect_incrementer_result.status_value(), ZX_OK);

  zx::result<> connect_incrementer_using_server_end_result =
      incoming->Connect<test_display_namespace::MultiProtocolService::Incrementer>(
          std::move(incrementer_server_end), "single-protocol-fragment");
  ASSERT_NE(connect_incrementer_using_server_end_result.status_value(), ZX_OK);
}

}  // namespace

}  // namespace display
