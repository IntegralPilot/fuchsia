// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/trusted_flatland_factory.h"

#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/fidl/cpp/client.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/ui/scenic/lib/allocation/mock_buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/flatland_manager.h"
#include "src/ui/scenic/lib/flatland/tests/logging_event_loop.h"
#include "src/ui/scenic/lib/flatland/tests/mock_flatland_presenter.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

using flatland::FlatlandManager;
using flatland::LinkSystem;
using flatland::MockFlatlandPresenter;
using flatland::TrustedFlatlandFactoryImpl;
using flatland::UberStructSystem;
using ::testing::_;

namespace {

class TrustedFlatlandFactoryTest : public LoggingEventLoop, public ::testing::Test {
 public:
  TrustedFlatlandFactoryTest() : dispatcher_setter_(this->dispatcher(), this->dispatcher()) {}

  void SetUp() override {
    ::testing::Test::SetUp();

    mock_flatland_presenter_ = std::make_shared<::testing::StrictMock<MockFlatlandPresenter>>();
    EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(_, _)).Times(::testing::AtLeast(0));
    ON_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _))
        .WillByDefault(::testing::Invoke(
            [&](zx::time requested_presentation_time, scheduling::SchedulingIdPair id_pair,
                bool unsquashable, std::vector<zx::event> release_fences,
                std::vector<zx::counter> release_counters, std::vector<zx::counter> present_fences,
                bool schedule_asap) {}));

    const display::WireDisplayId kDisplayId = {.value = 1};
    static constexpr uint32_t kMaxDisplayLayersCount = 2;
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> importers;
    importers.push_back(std::make_shared<allocation::MockBufferCollectionImporter>());
    flatland_manager_ = std::make_shared<FlatlandManager>(
        this->dispatcher(), mock_flatland_presenter_, uber_struct_system_, link_system_,
        std::make_shared<display::Display>(kDisplayId, 640, 480, kMaxDisplayLayersCount), importers,
        /*register_view_focuser*/ [](auto...) {},
        /*register_view_ref_focused*/ [](auto...) {},
        /*register_touch_source*/ [](auto...) {},
        /*register_mouse_source*/ [](auto...) {},
        /*register_touch_source_v2*/ [](auto...) {},
        /*register_mouse_source_v2*/ [](auto...) {});

    factory_ = std::make_unique<TrustedFlatlandFactoryImpl>(flatland_manager_);
  }

  void TearDown() override {
    factory_.reset();
    flatland_manager_.reset();
    mock_flatland_presenter_.reset();
    flatland_clients_.clear();
    RunLoopUntilIdle();
    ::testing::Test::TearDown();
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_;
  std::unique_ptr<TrustedFlatlandFactoryImpl> factory_;
  std::shared_ptr<FlatlandManager> flatland_manager_;
  std::vector<fidl::Client<fuchsia_ui_composition::Flatland>> flatland_clients_;

 private:
  std::shared_ptr<MockFlatlandPresenter> mock_flatland_presenter_;
  const std::shared_ptr<UberStructSystem> uber_struct_system_ =
      std::make_shared<UberStructSystem>();
  const std::shared_ptr<LinkSystem> link_system_ =
      std::make_shared<LinkSystem>(uber_struct_system_->GetNextInstanceId());
};

}  // namespace

namespace flatland {

TEST_F(TrustedFlatlandFactoryTest, CreateFlatland) {
  // Check that no Flatland instances exist initially.
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 0ul);

  // Create a client endpoint for the factory.
  auto endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::TrustedFlatlandFactory>();
  ASSERT_TRUE(endpoints.is_ok());
  factory_->GetHandler()(std::move(endpoints->server));
  fidl::Client factory_client(std::move(endpoints->client), this->dispatcher());

  // Create a client endpoint for the Flatland instance.
  fuchsia_ui_composition::TrustedFlatlandConfig config;
  auto flatland_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
  ASSERT_TRUE(flatland_endpoints.is_ok());

  // Call CreateFlatland on the factory.
  fuchsia_ui_composition::TrustedFlatlandFactoryCreateFlatlandRequest request;
  request.server_end() = std::move(flatland_endpoints->server);
  request.config() = std::move(config);
  factory_client->CreateFlatland(std::move(request)).Then([](auto& result) {
    ASSERT_TRUE(result.is_ok());
  });

  // Create the client immediately to keep the channel open.
  flatland_clients_.emplace_back(std::move(flatland_endpoints->client), this->dispatcher());
  EXPECT_TRUE(flatland_clients_.back().is_valid());

  RunLoopUntilIdle();

  // Check that a new Flatland instance was created and is still alive.
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 1ul);

  auto session_ids = flatland_manager_->GetSessionIdsForTest();
  ASSERT_EQ(session_ids.size(), 1ul);

  // Assert that the Flatland instance is running on scenic's main thread.
  EXPECT_EQ(flatland_manager_->GetSessionDispatcherForTest(session_ids[0]), this->dispatcher());
}

TEST_F(TrustedFlatlandFactoryTest, ToInternalConfig) {
  // Test default values (all absent).
  {
    fuchsia_ui_composition::TrustedFlatlandConfig config;
    auto internal_config = TrustedFlatlandFactoryImpl::ToInternalConfig(std::move(config));
    EXPECT_FALSE(internal_config.schedule_asap);
    EXPECT_FALSE(internal_config.pass_acquire_fences);
    EXPECT_FALSE(internal_config.skips_present_credits);
    EXPECT_FALSE(internal_config.skips_on_frame_presented);
    EXPECT_TRUE(internal_config.use_trusted_flatland_api);
  }

  // Test all true.
  {
    fuchsia_ui_composition::TrustedFlatlandConfig config;
    config.schedule_asap() = true;
    config.pass_acquire_fences() = true;
    config.skips_present_credits() = true;
    config.skips_on_frame_presented() = true;

    auto internal_config = TrustedFlatlandFactoryImpl::ToInternalConfig(std::move(config));
    EXPECT_TRUE(internal_config.schedule_asap);
    EXPECT_TRUE(internal_config.pass_acquire_fences);
    EXPECT_TRUE(internal_config.skips_present_credits);
    EXPECT_TRUE(internal_config.skips_on_frame_presented);
    EXPECT_TRUE(internal_config.use_trusted_flatland_api);
  }

  // Test mix.
  {
    fuchsia_ui_composition::TrustedFlatlandConfig config;
    config.schedule_asap() = true;
    config.skips_on_frame_presented() = false;

    auto internal_config = TrustedFlatlandFactoryImpl::ToInternalConfig(std::move(config));
    EXPECT_TRUE(internal_config.schedule_asap);
    EXPECT_FALSE(internal_config.pass_acquire_fences);
    EXPECT_FALSE(internal_config.skips_present_credits);
    EXPECT_FALSE(internal_config.skips_on_frame_presented);
    EXPECT_TRUE(internal_config.use_trusted_flatland_api);
  }
}

}  // namespace flatland
