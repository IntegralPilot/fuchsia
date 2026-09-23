// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver/metadata/cpp/tests/metadata_integration_test/metadata_forwarder_test_driver/metadata_forwarder_test_driver.h>
#include <lib/driver/metadata/cpp/tests/metadata_integration_test/metadata_retriever_test_driver/metadata_retriever_test_driver.h>
#include <lib/driver/metadata/cpp/tests/metadata_integration_test/metadata_sender_test_driver/metadata_sender_test_driver.h>
#include <lib/driver/metadata/cpp/tests/metadata_integration_test/test_root/test_root.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/fd.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_metadata::test {

class MetadataTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    loop_.StartThread();
    const std::vector<fuchsia_component_test::Capability> exposes = {
        {
            fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service(
                {.name = fuchsia_hardware_test::RootService::Name})),
        },
        {
            fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service(
                {.name = fuchsia_hardware_test::MetadataRetrieverService::Name})),
        },
        {
            fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service(
                {.name = fuchsia_hardware_test::MetadataSenderService::Name})),
        }};
    // Create and build the realm.
    auto realm_builder = component_testing::RealmBuilder::Create();
    driver_test_realm::Setup(realm_builder, loop_.dispatcher(),
                             driver_test_realm::OptionsBuilder().driver_exposes(exposes).Build(),
                             fuchsia_driver_test::RealmArgs{{.root_driver = "#meta/test_root.cm"}});
    realm_.emplace(realm_builder.Build(loop_.dispatcher()));
    auto boot_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_EQ(ZX_OK, boot_result.status_value());

    // Connect to root service.
    component::SyncServiceMemberWatcher<fuchsia_hardware_test::RootService::Device> watcher(
        SvcDir());
    zx::result sender = watcher.GetNextInstance(false);
    ASSERT_OK(sender);
    root_.Bind(std::move(sender.value()));
  }

  void TearDown() override { driver_test_realm::ShutdownRealm(*realm_); }

 protected:
  // |expose| determines if the sender driver will expose the metadata
  // FIDL service in its component manifest.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> CreateSender(bool expose) const {
    fidl::Result result = root_->AddMetadataSenderNode({{.exposes_metadata_fidl_service = expose}});
    EXPECT_TRUE(result.is_ok());

    component::SyncServiceMemberWatcher<fuchsia_hardware_test::MetadataSenderService::Device>
        watcher(SvcDir());
    zx::result client = watcher.GetNextInstance(false);
    EXPECT_OK(client);
    return fidl::SyncClient<fuchsia_hardware_test::MetadataSender>(std::move(client.value()));
  }

  // |use| determines if the retriever driver
  // declares that it uses the metadata FIDL service in its component manifest. Return a client for
  // the sender driver and retriever driver.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> CreateRetriever(
      fidl::SyncClient<fuchsia_hardware_test::MetadataSender>& sender, bool use) const {
    fidl::Result result = sender->AddMetadataRetrieverNode({{.uses_metadata_fidl_service = use}});
    EXPECT_TRUE(result.is_ok());

    component::SyncServiceMemberWatcher<fuchsia_hardware_test::MetadataRetrieverService::Device>
        watcher(SvcDir());
    zx::result client = watcher.GetNextInstance(false);
    EXPECT_OK(client);
    return fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>(std::move(client.value()));
  }

  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> CreateForwarderAndRetriever(
      fidl::SyncClient<fuchsia_hardware_test::MetadataSender>& sender) const {
    fidl::Result result = sender->AddMetadataForwarderNode();
    EXPECT_TRUE(result.is_ok());

    component::SyncServiceMemberWatcher<fuchsia_hardware_test::MetadataRetrieverService::Device>
        watcher(SvcDir());
    zx::result client = watcher.GetNextInstance(false);
    EXPECT_OK(client);
    return fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever>(std::move(client.value()));
  }

  fidl::SyncClient<fuchsia_hardware_test::Root>& root() { return root_; }

 private:
  fidl::UnownedClientEnd<fuchsia_io::Directory> SvcDir() const {
    return fidl::UnownedClientEnd<fuchsia_io::Directory>(
        realm_->component().exposed().unowned_channel()->get());
  }

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::optional<component_testing::RealmRoot> realm_;
  fidl::SyncClient<fuchsia_hardware_test::Root> root_;
};

// Verify that `fdf_metadata::MetadataServer` can serve metadata from a node and that one of the
// node's child nodes can retrieve the metadata using `fdf::GetMetadata()`.
TEST_F(MetadataTest, SendAndRetrieveMetadata) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "arbitrary"});

  // Set sender driver's metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(true);
  {
    fidl::Result result = sender->ServeMetadata(kMetadata);
    ASSERT_TRUE(result.is_ok());
  }

  // Make the retriever get metadata from its parent (which is sender). Verify
  // that the metadata is the same as the metadata assigned to the sender driver instance.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateRetriever(sender, true);
  {
    fidl::Result result = retriever->GetMetadata();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_EQ(result.value().metadata(), kMetadata);
  }
}

// Verify that a driver can retrieve metadata with `fdf_metadata::GetMetadataIfExists()`.
TEST_F(MetadataTest, GetMetadataIfExists) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "arbitrary"});

  // Set sender driver's metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(true);
  {
    fidl::Result result = sender->ServeMetadata(kMetadata);
    ASSERT_TRUE(result.is_ok());
  }

  // Make the retriever get metadata from its parent (which is sender). Verify
  // that the metadata is the same as the metadata assigned to the sender driver instance.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateRetriever(sender, true);
  {
    fidl::Result result = retriever->GetMetadataIfExists();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_TRUE(result.value().retrieved_metadata());
    ASSERT_EQ(result.value().metadata(), kMetadata);
  }
}

// Verify that `fdf_metadata::GetMetadataIfExists()` will return std::nullopt instead of an error
// when there are no metadata servers exposed in the calling driver's incoming namespace.
TEST_F(MetadataTest, GetMetadataIfExistsNullopt) {
  // Make the retriever get metadata from its parent (which is sender). Verify
  // that the metadata is the same as the metadata assigned to the sender driver instance.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(true);
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateRetriever(sender, true);
  {
    fidl::Result result = retriever->GetMetadataIfExists();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_FALSE(result.value().retrieved_metadata());
  }
}

// Verify that a driver can forward metadata using
// `fdf_metadata::MetadataServer::ForwardMetadata()`.
TEST_F(MetadataTest, ForwardMetadata) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "arbitrary"});

  // Create a sender driver instance and serve metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(true);
  {
    fidl::Result result = sender->ServeMetadata(kMetadata);
    ASSERT_TRUE(result.is_ok());
  }

  // Make the retriever get metadata from its parent which is a sender driver.
  // Verify that the metadata is the same as the metadata assigned to the sender
  // driver.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateForwarderAndRetriever(sender);
  {
    fidl::Result result = retriever->GetMetadata();
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_EQ(result.value().metadata(), kMetadata);
  }
}

// Verify that a driver is unable to retrieve metadata via `fdf::GetMetadata()` if the driver does
// not specify in its component manifest that it uses the metadata FIDL service.
TEST_F(MetadataTest, FailMetadataTransferWithExposeAndNoUse) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "arbitrary"});

  // Set sender driver's metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(true);
  {
    fidl::Result result = sender->ServeMetadata(kMetadata);
    ASSERT_TRUE(result.is_ok());
  }

  // Make the retriever driver fail to retrieve metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateRetriever(sender, false);
  {
    fidl::Result result = retriever->GetMetadata();
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_PEER_CLOSED);
  }
}

// Verify that a driver is unable to retrieve metadata via `fdf::GetMetadata()` if the parent driver
// does not expose the metadata FIDL service in its component manifest.
TEST_F(MetadataTest, FailMetadataTransferWithNoExposeButUse) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "arbitrary"});

  // Set sender driver's metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(false);
  {
    fidl::Result result = sender->ServeMetadata(kMetadata);
    ASSERT_TRUE(result.is_ok());
  }

  // Make the retriever driver fail to retrieve metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateRetriever(sender, true);
  {
    fidl::Result result = retriever->GetMetadata();
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_PEER_CLOSED);
  }
}

// Verify that a driver is unable to retrieve metadata via `fdf::GetMetadata()` if the driver does
// not specify in its component manifest that it uses the metadata FIDL service and the parent
// driver does not expose the metadata FIDL service in its component manifest.
TEST_F(MetadataTest, FailMetadataTransferWithNoExposeAndNoUse) {
  static const fuchsia_hardware_test::Metadata kMetadata({.test_property = "arbitrary"});

  // Set sender driver's metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataSender> sender = CreateSender(false);
  {
    fidl::Result result = sender->ServeMetadata(kMetadata);
    ASSERT_TRUE(result.is_ok());
  }

  // Make the retriever driver fail to retrieve metadata.
  fidl::SyncClient<fuchsia_hardware_test::MetadataRetriever> retriever =
      CreateRetriever(sender, false);
  {
    fidl::Result result = retriever->GetMetadata();
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_PEER_CLOSED);
  }
}

}  // namespace fdf_metadata::test

#endif
