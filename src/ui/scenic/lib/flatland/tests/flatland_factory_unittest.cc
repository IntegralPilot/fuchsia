// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_factory.h"

#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/fidl/cpp/client.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/ui/scenic/lib/allocation/mock_buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/flatland_manager.h"
#include "src/ui/scenic/lib/flatland/tests/logging_event_loop.h"
#include "src/ui/scenic/lib/flatland/tests/mock_flatland_presenter.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

using flatland::FlatlandFactoryImpl;
using flatland::FlatlandManager;
using flatland::LinkSystem;
using flatland::MockFlatlandPresenter;
using flatland::UberStructSystem;
using ::testing::_;

namespace {

class FlatlandFactoryTest : public LoggingEventLoop, public ::testing::Test {
 public:
  FlatlandFactoryTest() : dispatcher_setter_(this->dispatcher(), this->dispatcher()) {}

  void SetUp() override {
    ::testing::Test::SetUp();

    mock_flatland_presenter_ = std::make_shared<::testing::StrictMock<MockFlatlandPresenter>>();
    EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(_, _)).Times(::testing::AtLeast(0));

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

    factory_ = std::make_unique<FlatlandFactoryImpl>(flatland_manager_);
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
  std::unique_ptr<FlatlandFactoryImpl> factory_;
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

TEST_F(FlatlandFactoryTest, CreateFlatland) {
  // Check that no Flatland instances exist initially.
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 0ul);

  // Create a client endpoint for the factory.
  auto endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::FlatlandFactory>();
  ASSERT_TRUE(endpoints.is_ok());
  factory_->GetHandler()(std::move(endpoints->server));
  fidl::Client factory_client(std::move(endpoints->client), this->dispatcher());

  // Create a client endpoint for the Flatland instance.
  fuchsia_ui_composition::FlatlandConfig config;
  auto flatland_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
  ASSERT_TRUE(flatland_endpoints.is_ok());

  // Call CreateFlatland on the factory.
  fuchsia_ui_composition::FlatlandFactoryCreateFlatlandRequest request;
  request.server_end() = std::move(flatland_endpoints->server);
  request.config() = std::move(config);
  bool create_ok = false;
  factory_client->CreateFlatland(std::move(request))
      .Then(
          [&create_ok](fidl::Result<fuchsia_ui_composition::FlatlandFactory::CreateFlatland>& res) {
            create_ok = res.is_ok();
          });

  // Create the client immediately to keep the channel open.
  flatland_clients_.emplace_back(std::move(flatland_endpoints->client), this->dispatcher());
  EXPECT_TRUE(flatland_clients_.back().is_valid());

  RunLoopUntilIdle();

  EXPECT_TRUE(create_ok);
  // Check that a new Flatland instance was created and is still alive.
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 1ul);

  auto session_ids = flatland_manager_->GetSessionIdsForTest();
  ASSERT_EQ(session_ids.size(), 1ul);
}

TEST_F(FlatlandFactoryTest, ToInternalConfig) {
  // Test default values (use_flatland2 absent or false).
  {
    fuchsia_ui_composition::FlatlandConfig config;
    auto internal_config = FlatlandFactoryImpl::ToInternalConfig(std::move(config));
    EXPECT_FALSE(internal_config.skips_on_frame_presented);
    EXPECT_FALSE(internal_config.use_flatland2);
    EXPECT_FALSE(internal_config.use_trusted_flatland_api);
  }

  // Test use_flatland2 = true.
  {
    fuchsia_ui_composition::FlatlandConfig config;
    config.use_flatland2() = true;

    auto internal_config = FlatlandFactoryImpl::ToInternalConfig(std::move(config));
    EXPECT_TRUE(internal_config.skips_on_frame_presented);
    EXPECT_TRUE(internal_config.use_flatland2);
    EXPECT_FALSE(internal_config.use_trusted_flatland_api);
  }
}

TEST_F(FlatlandFactoryTest, MultipleSessionsSingleFactoryConnection) {
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 0ul);

  auto factory_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::FlatlandFactory>();
  ASSERT_TRUE(factory_endpoints.is_ok());
  factory_->GetHandler()(std::move(factory_endpoints->server));
  fidl::Client factory_client(std::move(factory_endpoints->client), this->dispatcher());

  constexpr size_t kNumSessions = 3;
  for (size_t i = 0; i < kNumSessions; ++i) {
    auto flatland_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
    ASSERT_TRUE(flatland_endpoints.is_ok());

    fuchsia_ui_composition::FlatlandFactoryCreateFlatlandRequest request;
    request.server_end() = std::move(flatland_endpoints->server);
    request.config() = fuchsia_ui_composition::FlatlandConfig();
    factory_client->CreateFlatland(std::move(request)).Then([](auto& res) {
      EXPECT_TRUE(res.is_ok());
    });

    flatland_clients_.emplace_back(std::move(flatland_endpoints->client), this->dispatcher());
  }

  RunLoopUntilIdle();
  EXPECT_EQ(flatland_manager_->GetSessionCount(), kNumSessions);
  EXPECT_EQ(flatland_manager_->GetSessionIdsForTest().size(), kNumSessions);
}

TEST_F(FlatlandFactoryTest, MultipleConcurrentFactoryConnections) {
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 0ul);

  auto factory_endpoints_1 = fidl::CreateEndpoints<fuchsia_ui_composition::FlatlandFactory>();
  ASSERT_TRUE(factory_endpoints_1.is_ok());
  factory_->GetHandler()(std::move(factory_endpoints_1->server));
  fidl::Client factory_client_1(std::move(factory_endpoints_1->client), this->dispatcher());

  auto factory_endpoints_2 = fidl::CreateEndpoints<fuchsia_ui_composition::FlatlandFactory>();
  ASSERT_TRUE(factory_endpoints_2.is_ok());
  factory_->GetHandler()(std::move(factory_endpoints_2->server));
  fidl::Client factory_client_2(std::move(factory_endpoints_2->client), this->dispatcher());

  // Session from factory 1
  {
    auto flatland_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
    ASSERT_TRUE(flatland_endpoints.is_ok());
    fuchsia_ui_composition::FlatlandFactoryCreateFlatlandRequest request;
    request.server_end() = std::move(flatland_endpoints->server);
    request.config() = fuchsia_ui_composition::FlatlandConfig();
    factory_client_1->CreateFlatland(std::move(request)).Then([](auto& res) {
      EXPECT_TRUE(res.is_ok());
    });
    flatland_clients_.emplace_back(std::move(flatland_endpoints->client), this->dispatcher());
  }

  // Session from factory 2
  {
    auto flatland_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
    ASSERT_TRUE(flatland_endpoints.is_ok());
    fuchsia_ui_composition::FlatlandFactoryCreateFlatlandRequest request;
    request.server_end() = std::move(flatland_endpoints->server);
    request.config() = fuchsia_ui_composition::FlatlandConfig();
    factory_client_2->CreateFlatland(std::move(request)).Then([](auto& res) {
      EXPECT_TRUE(res.is_ok());
    });
    flatland_clients_.emplace_back(std::move(flatland_endpoints->client), this->dispatcher());
  }

  RunLoopUntilIdle();
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 2ul);
}

TEST_F(FlatlandFactoryTest, SessionCleanupOnClientClosure) {
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 0ul);

  auto factory_endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::FlatlandFactory>();
  ASSERT_TRUE(factory_endpoints.is_ok());
  factory_->GetHandler()(std::move(factory_endpoints->server));
  fidl::Client factory_client(std::move(factory_endpoints->client), this->dispatcher());

  // Session A
  auto flatland_endpoints_a = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
  ASSERT_TRUE(flatland_endpoints_a.is_ok());
  {
    fuchsia_ui_composition::FlatlandFactoryCreateFlatlandRequest request;
    request.server_end() = std::move(flatland_endpoints_a->server);
    request.config() = fuchsia_ui_composition::FlatlandConfig();
    factory_client->CreateFlatland(std::move(request)).Then([](auto& res) {
      EXPECT_TRUE(res.is_ok());
    });
  }
  fidl::Client client_a(std::move(flatland_endpoints_a->client), this->dispatcher());

  // Session B
  auto flatland_endpoints_b = fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>();
  ASSERT_TRUE(flatland_endpoints_b.is_ok());
  {
    fuchsia_ui_composition::FlatlandFactoryCreateFlatlandRequest request;
    request.server_end() = std::move(flatland_endpoints_b->server);
    request.config() = fuchsia_ui_composition::FlatlandConfig();
    factory_client->CreateFlatland(std::move(request)).Then([](auto& res) {
      EXPECT_TRUE(res.is_ok());
    });
  }
  fidl::Client client_b(std::move(flatland_endpoints_b->client), this->dispatcher());

  RunLoopUntilIdle();
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 2ul);

  // Close client_a channel and verify session count drops to 1.
  client_a = {};
  RunLoopUntil([this] { return flatland_manager_->GetSessionCount() == 1ul; });
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 1ul);

  // Closing factory client should not terminate remaining active session (client_b).
  factory_client = {};
  RunLoopUntilIdle();
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 1ul);

  // Close client_b channel and verify all sessions are cleaned up.
  client_b = {};
  RunLoopUntil([this] { return flatland_manager_->GetSessionCount() == 0ul; });
  EXPECT_EQ(flatland_manager_->GetSessionCount(), 0ul);
}

}  // namespace flatland
