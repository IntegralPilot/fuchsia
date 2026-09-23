// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <ddktl/metadata_server.h>
#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/ddktl/tests/metadata-server-test/metadata-retriever-test-driver/metadata_retriever_test_device.h"
#include "src/lib/testing/predicates/status.h"

namespace ddk::test {

class IncomingNamespace {
 public:
  void Init(fidl::ServerEnd<fuchsia_io::Directory> outgoing_server) {
    ASSERT_OK(metadata_server_.Serve(outgoing_, async_get_default_dispatcher()));
    ASSERT_OK(outgoing_.Serve(std::move(outgoing_server)));
  }

  void SetMetadata(const fuchsia_hardware_test::wire::Metadata& metadata) {
    metadata_server_.SetMetadata(metadata);
  }

 private:
  ddk::MetadataServer<fuchsia_hardware_test::wire::Metadata> metadata_server_;
  component::OutgoingDirectory outgoing_{async_get_default_dispatcher()};
};

class MetadataServerMockDdkTest : public ::testing::Test {
 public:
  void SetUp() override {
    // Set-up driver's incoming namespace.
    ASSERT_OK(incoming_loop_.StartThread("incoming-namespace"));
    auto incoming_namespace_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_namespace_.SyncCall(&IncomingNamespace::Init,
                                 std::move(incoming_namespace_endpoints.server));
    parent_->AddFidlService(MetadataServer<fuchsia_hardware_test::Metadata>::kFidlServiceName,
                            std::move(incoming_namespace_endpoints.client), "default");

    // Start driver.
    auto result = fdf::RunOnDispatcherSync(driver_dispatcher_->async_dispatcher(), [&]() {
      ASSERT_OK(MetadataRetrieverTestDevice::Bind(nullptr, parent_.get()));
    });
    ASSERT_OK(result);

    // Connect to metadata retriever driver's FIDL protocol.
    auto metadata_retriever_endpoints =
        fidl::Endpoints<fuchsia_hardware_test::MetadataRetriever>::Create();
    fidl::BindServer(driver_dispatcher_->async_dispatcher(),
                     std::move(metadata_retriever_endpoints.server),
                     parent_->GetLatestChild()->GetDeviceContext<MetadataRetrieverTestDevice>());
    client_.Bind(std::move(metadata_retriever_endpoints.client));
  }

 protected:
  void SetMetadata(const fuchsia_hardware_test::wire::Metadata& metadata) {
    incoming_namespace_.SyncCall(&IncomingNamespace::SetMetadata, metadata);
  }

  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>& client() { return client_; }

 private:
  std::shared_ptr<MockDevice> parent_ = MockDevice::FakeRootParent();
  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_namespace_{
      incoming_loop_.dispatcher(), std::in_place};
  fdf::UnownedSynchronizedDispatcher driver_dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
  fidl::ServerBindingGroup<fuchsia_hardware_test::MetadataRetriever> bindings_;
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> client_;
};

// Verify that a driver can retrieve metadata via `ddk::GetMetadata()` from a mock parent device
// created by the mock-ddk library.
TEST_F(MetadataServerMockDdkTest, GetMetadata) {
  const char* kTestPropertyValue = "test-value";

  // Set the metadata to be retrieved.
  fidl::Arena arena;
  auto metadata = fuchsia_hardware_test::wire::Metadata::Builder(arena)
                      .test_property(kTestPropertyValue)
                      .Build();
  SetMetadata(metadata);

  // Get the metadata.
  fidl::Result result = client()->GetMetadata();
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  auto test_property = result->metadata().test_property();
  ASSERT_TRUE(test_property.has_value());
  ASSERT_EQ(test_property.value(), kTestPropertyValue);
}

}  // namespace ddk::test
