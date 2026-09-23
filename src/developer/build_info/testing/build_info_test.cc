// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "build_info.h"

#include <fidl/fuchsia.buildinfo.test/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {
using component_testing::RealmBuilder;
using component_testing::RealmRoot;

using fuchsia_buildinfo::BuildInfo;
using fuchsia_buildinfo::Provider;
using fuchsia_buildinfo_test::BuildInfoTestController;

using component_testing::ChildRef;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Route;
}  // namespace

class FakeBuildInfoTestFixture : public gtest::RealLoopFixture {
 public:
  static constexpr char fake_provider_url[] =
      "fuchsia-pkg://fuchsia.com/fake_build_info_test#meta/fake_build_info.cm";
  static constexpr char fake_provider_name[] = "fake_provider";

  static constexpr auto kProductName = "workstation";
  static constexpr auto kBoardName = "x64";
  static constexpr auto kVersion = "2022-03-28T15:42:20+00:00";
  static constexpr auto kPlatformVersion = "2024-03-28T15:42:20+00:00";
  static constexpr auto kProductVersion = "2024-04-28T15:42:20+00:00";
  static constexpr auto kLastCommitDate = "2022-03-28T15:42:20+00:00";

  FakeBuildInfoTestFixture()
      : realm_builder_(std::make_unique<RealmBuilder>(RealmBuilder::Create())) {}

  void SetUp() override {
    SetUpRealm(realm_builder_.get());

    realm_ = std::make_unique<RealmRoot>(realm_builder_->Build(dispatcher()));
  }

 protected:
  void SetUpRealm(RealmBuilder* builder) {
    realm_builder_->AddChild(fake_provider_name, fake_provider_url);

    realm_builder_->AddRoute(
        Route{.capabilities = {Protocol{fidl::DiscoverableProtocolName<Provider>},
                               Protocol{fidl::DiscoverableProtocolName<BuildInfoTestController>}},
              .source = ChildRef{fake_provider_name},
              .targets = {ParentRef()}});
  }

  RealmRoot* realm() { return realm_.get(); }

 private:
  std::unique_ptr<RealmRoot> realm_;
  std::unique_ptr<RealmBuilder> realm_builder_;
};

TEST_F(FakeBuildInfoTestFixture, SetBuildInfo) {
  auto client_end = realm()->component().Connect<Provider>();
  ASSERT_TRUE(client_end.is_ok());
  auto provider = fidl::SyncClient<Provider>(std::move(*client_end));

  auto test_controller_client_end = realm()->component().Connect<BuildInfoTestController>();
  ASSERT_TRUE(test_controller_client_end.is_ok());
  auto test_controller =
      fidl::SyncClient<BuildInfoTestController>(std::move(*test_controller_client_end));

  auto result = provider->GetBuildInfo();
  ASSERT_TRUE(result.is_ok());
  auto build_info = result.value().build_info();

  EXPECT_TRUE(build_info.product_config().has_value());
  EXPECT_EQ(build_info.product_config().value(), FakeProviderImpl::kProductNameDefault);
  EXPECT_TRUE(build_info.board_config().has_value());
  EXPECT_EQ(build_info.board_config().value(), FakeProviderImpl::kBoardNameDefault);
  EXPECT_TRUE(build_info.version().has_value());
  EXPECT_EQ(build_info.version().value(), FakeProviderImpl::kVersionDefault);
  EXPECT_TRUE(build_info.latest_commit_date().has_value());
  EXPECT_EQ(build_info.latest_commit_date().value(), FakeProviderImpl::kLastCommitDateDefault);

  BuildInfo new_build_info;
  new_build_info.board_config(FakeBuildInfoTestFixture::kBoardName);
  new_build_info.product_config(FakeBuildInfoTestFixture::kProductName);
  new_build_info.version(FakeBuildInfoTestFixture::kVersion);
  new_build_info.platform_version(FakeBuildInfoTestFixture::kPlatformVersion);
  new_build_info.product_version(FakeBuildInfoTestFixture::kProductVersion);
  new_build_info.latest_commit_date(FakeBuildInfoTestFixture::kLastCommitDate);

  auto set_result = test_controller->SetBuildInfo({{.build_info = std::move(new_build_info)}});
  ASSERT_TRUE(set_result.is_ok());

  result = provider->GetBuildInfo();
  ASSERT_TRUE(result.is_ok());
  build_info = result.value().build_info();

  EXPECT_TRUE(build_info.product_config().has_value());
  EXPECT_EQ(build_info.product_config().value(), FakeBuildInfoTestFixture::kProductName);
  EXPECT_TRUE(build_info.board_config().has_value());
  EXPECT_EQ(build_info.board_config().value(), FakeBuildInfoTestFixture::kBoardName);
  EXPECT_TRUE(build_info.version().has_value());
  EXPECT_EQ(build_info.version().value(), FakeBuildInfoTestFixture::kVersion);
  EXPECT_TRUE(build_info.platform_version().has_value());
  EXPECT_EQ(build_info.platform_version().value(), FakeBuildInfoTestFixture::kPlatformVersion);
  EXPECT_TRUE(build_info.product_version().has_value());
  EXPECT_EQ(build_info.product_version().value(), FakeBuildInfoTestFixture::kProductVersion);
  EXPECT_TRUE(build_info.latest_commit_date().has_value());
  EXPECT_EQ(build_info.latest_commit_date().value(), FakeBuildInfoTestFixture::kLastCommitDate);
}
