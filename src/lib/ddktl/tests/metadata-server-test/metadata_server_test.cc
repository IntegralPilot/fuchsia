// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/fd.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace ddk::test {

class MetadataServerTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    loop_.StartThread();

    // Create and build the realm.
    auto realm_builder = component_testing::RealmBuilder::Create();
    driver_test_realm::Setup(
        realm_builder, loop_.dispatcher(), {},
        fuchsia_driver_test::RealmArgs{
            {.root_driver = "fuchsia-boot:///dtr#meta/metadata_sender_test_driver.cm"}});
    realm_.emplace(realm_builder.Build(loop_.dispatcher()));

    // Start DriverTestRealm.
    zx::result<> boot_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_EQ(ZX_OK, boot_result.status_value());

    // Connect to devfs directory.
    // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(realm_->component().exposed()->Open("dev-topological", fuchsia::io::PERM_READABLE, {},
                                                  server.TakeChannel()),
              ZX_OK);
    ASSERT_EQ(fdio_fd_create(client.TakeChannel().release(), &dev_fd_), ZX_OK);
  }

  void TearDown() override { driver_test_realm::ShutdownRealm(*realm_); }

 protected:
  // Make the metadata_sender driver create a child device that the metadata_retriever driver can
  // bind to. Returns a connection to the metadata_sender driver and metadata_retriever driver.
  std::pair<fidl::SyncClient<fuchsia_hardware_test::MetadataSender>,
            fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>>
  CreateMetadataSenderAndRetriever(bool expose_metadata_to_retriever) {
    fidl::SyncClient metadata_sender =
        ConnectToDevice<fuchsia_hardware_test::MetadataSender>("metadata_sender");

    // Create a child device for the metadata_retriever driver to bind to.
    fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> metadata_retriever;
    {
      fidl::Result device_name = metadata_sender->AddMetadataRetrieverDevice(
          {{.expose_metadata = expose_metadata_to_retriever}});
      EXPECT_TRUE(device_name.is_ok()) << device_name.error_value();
      std::string metadata_retriever_path = std::string{"metadata_sender/"}
                                                .append(device_name->child_device_name())
                                                .append("/")
                                                .append("metadata_retriever");
      metadata_retriever =
          ConnectToDevice<fuchsia_hardware_test::MetadataRetriever>(metadata_retriever_path);
    }

    return std::pair<fidl::SyncClient<fuchsia_hardware_test::MetadataSender>,
                     fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>>(
        std::move(metadata_sender), std::move(metadata_retriever));
  }

  // Make the metadata_sender driver create a child device that the metadata_forwarder driver can
  // bind to. The metadata_forwarder driver will create a child device that the metadata_retriever
  // driver can bind to. Returns a connection to the metadata_sender driver, metadata_forwarder
  // driver, and the metadata_retriever driver that is bound to metadata_forwarder's child device.
  std::tuple<fidl::SyncClient<fuchsia_hardware_test::MetadataSender>,
             fidl::SyncClient<fuchsia_hardware_test::MetadataForwarder>,
             fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>>
  CreateMetadataSenderForwarderAndRetriever() {
    fidl::SyncClient metadata_sender =
        ConnectToDevice<fuchsia_hardware_test::MetadataSender>("metadata_sender");

    // Create a child device for the metadata_forwarder driver to bind to. The metadat_forwarder
    // driver will create a child device for the metadata_retriever driver to bind to.
    fidl::SyncClient<fuchsia_hardware_test::MetadataForwarder> metadata_forwarder;
    fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> metadata_retriever;
    {
      fidl::Result metadata_forwarder_device_name = metadata_sender->AddMetadataForwarderDevice();
      EXPECT_TRUE(metadata_forwarder_device_name.is_ok())
          << metadata_forwarder_device_name.error_value();
      std::string metadata_forwarder_path =
          std::string{"metadata_sender/"}
              .append(metadata_forwarder_device_name->child_device_name())
              .append("/")
              .append("metadata_forwarder");
      metadata_forwarder =
          ConnectToDevice<fuchsia_hardware_test::MetadataForwarder>(metadata_forwarder_path);

      std::string metadata_retriever_path =
          std::string{metadata_forwarder_path}.append("/metadata_retriever");
      metadata_retriever =
          ConnectToDevice<fuchsia_hardware_test::MetadataRetriever>(metadata_retriever_path);
    }

    return std::tuple<fidl::SyncClient<fuchsia_hardware_test::MetadataSender>,
                      fidl::SyncClient<fuchsia_hardware_test::MetadataForwarder>,
                      fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>>(
        std::move(metadata_sender), std::move(metadata_forwarder), std::move(metadata_retriever));
  }

 private:
  // Connect to the |FidlProtocol| FIDL protocol found at |path| within devfs.
  template <typename FidlProtocol>
  fidl::SyncClient<FidlProtocol> ConnectToDevice(std::string_view path) const {
    zx::result channel = device_watcher::RecursiveWaitForFile(dev_fd_, std::string{path}.c_str());
    EXPECT_EQ(channel.status_value(), ZX_OK) << path;
    return fidl::SyncClient{fidl::ClientEnd<FidlProtocol>{std::move(channel.value())}};
  }

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::optional<component_testing::RealmRoot> realm_;
  int dev_fd_;
};

// Verify that a driver can serve metadata with `ddk::MetadataServer` and that a child driver can
// retrieve that metadata with `ddk::GetMetadata()`.
TEST_F(MetadataServerTest, GetMetadata) {
  const char* kMetadataPropertyValue = "test property value";

  auto [metadata_sender, metadata_retriever] = CreateMetadataSenderAndRetriever(true);

  // Set the metadata provided by the metadata_sender driver.
  {
    fuchsia_hardware_test::Metadata metadata{{.test_property = kMetadataPropertyValue}};
    fidl::Result result = metadata_sender->SetMetadata(std::move(metadata));
    ASSERT_TRUE(result.is_ok()) << result.error_value();
  }

  // Make the metadata_retriever driver retrieve the metadata from its parent, the metadata_sender
  // driver, using `ddk::GetMetadata()`.
  {
    fidl::Result result = metadata_retriever->GetMetadata();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    fuchsia_hardware_test::Metadata metadata = std::move(result.value().metadata());
    ASSERT_EQ(metadata.test_property(), kMetadataPropertyValue);
  }
}

// Verify that `ddk::GetMetadata()` fails if the parent driver does not serve metadata to the
// calling driver.
TEST_F(MetadataServerTest, FailGetMetadata) {
  auto [metadata_sender, metadata_retriever] = CreateMetadataSenderAndRetriever(true);

  fidl::Result result = metadata_retriever->GetMetadata();
  ASSERT_TRUE(result.is_error());
}

// Verify that a driver can retrieve that metadata with `ddk::GetMetadataIfExists()`.
TEST_F(MetadataServerTest, GetMetadataIfExists) {
  const char* kMetadataPropertyValue = "test property value";

  auto [metadata_sender, metadata_retriever] = CreateMetadataSenderAndRetriever(true);

  // Set the metadata provided by the metadata_sender driver.
  {
    fuchsia_hardware_test::Metadata metadata{{.test_property = kMetadataPropertyValue}};
    fidl::Result result = metadata_sender->SetMetadata(std::move(metadata));
    ASSERT_TRUE(result.is_ok()) << result.error_value();
  }

  // Make the metadata_retriever driver retrieve the metadata from its parent, the metadata_sender
  // driver, using `ddk::GetMetadata()`.
  {
    fidl::Result result = metadata_retriever->GetMetadataIfExists();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().retrieved_metadata());
    fuchsia_hardware_test::Metadata metadata = std::move(result.value().metadata());
    ASSERT_EQ(metadata.test_property(), kMetadataPropertyValue);
  }
}

// Verify that `ddk::GetMetadataIfExists()` will return std::nullopt instead of an error when there
// are no metadata servers exposed in the calling driver's incoming namespace.
TEST_F(MetadataServerTest, GetMetadataIfExistsNullopt) {
  auto [metadata_sender, metadata_retriever] = CreateMetadataSenderAndRetriever(false);

  // Verify that the metadata_retriever driver's call to `ddk::GetMetadataIfExists()` returns a
  // std::nullopt and not an error or actual metadata.
  fidl::Result result = metadata_retriever->GetMetadataIfExists();
  ASSERT_TRUE(result.is_ok()) << result.error_value();
  ASSERT_FALSE(result.value().retrieved_metadata());
}

// Verify that a driver can forward metadata using `ddk::MetadataServer::ForwardMetadata()`.
TEST_F(MetadataServerTest, ForwardMetadata) {
  const char* kMetadataPropertyValue = "test property value";

  auto [metadata_sender, metadata_forwarder, metadata_retriever] =
      CreateMetadataSenderForwarderAndRetriever();

  // Set the metadata.
  {
    fuchsia_hardware_test::Metadata metadata{{.test_property = kMetadataPropertyValue}};
    fidl::Result result = metadata_sender->SetMetadata(std::move(metadata));
    ASSERT_TRUE(result.is_ok()) << result.error_value();
  }

  // Forward the metadata from the metadata_sender device to the metadata_retriever device using
  // `ddk::MetadataServer::ForwardMetadata()`.
  {
    fidl::Result result = metadata_forwarder->ForwardMetadata();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
  }

  // Verify that the metadata was successfully forwarded by the metadata_forwarder driver from the
  // metadata_sender driver to the metadata_retriever driver.
  {
    fidl::Result result = metadata_retriever->GetMetadata();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    fuchsia_hardware_test::Metadata metadata = std::move(result.value().metadata());
    ASSERT_EQ(metadata.test_property(), kMetadataPropertyValue);
  }
}

// Verify that `ddk::MetadataServer::ForwardMetadata()` fails if the parent driver does not serve
// metadata to the calling driver.
TEST_F(MetadataServerTest, FailForwardMetadata) {
  auto [metadata_sender, metadata_forwarder, metadata_retriever] =
      CreateMetadataSenderForwarderAndRetriever();

  fidl::Result result = metadata_forwarder->ForwardMetadata();
  ASSERT_TRUE(result.is_error());
}

}  //  namespace ddk::test
