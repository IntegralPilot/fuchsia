// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/vsync_source.h"

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/default.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/tests/mock_display_coordinator.h"
#include "src/ui/scenic/lib/display/vsync_source_manager.h"

namespace display::test {

static constexpr uint32_t kMaxDisplayLayersCount = 2;

class VsyncSourceTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    TestLoopFixture::SetUp();
    async_set_default_dispatcher(dispatcher());

    display_manager_ = std::make_unique<DisplayManager>([] {});
    vsync_source_manager_ = std::make_unique<VsyncSourceManager>(*display_manager_);

    const WireDisplayId kDisplayId = {.value = 1};
    display_manager_->SetDefaultDisplayForTests(
        std::make_shared<Display>(kDisplayId, 1024, 768, kMaxDisplayLayersCount));
  }

  void TearDown() override {
    vsync_source_manager_.reset();
    display_manager_.reset();
    TestLoopFixture::TearDown();
  }

  VsyncSourceManager* vsync_listener_manager() { return vsync_source_manager_.get(); }
  DisplayManager* display_manager() { return display_manager_.get(); }

 protected:
  size_t NumVsyncSourcesConnected() { return vsync_source_manager_->vsync_listeners_.size(); }

 private:
  std::unique_ptr<DisplayManager> display_manager_;
  std::unique_ptr<VsyncSourceManager> vsync_source_manager_;
};

class MockVsyncEventHandler
    : public fidl::WireAsyncEventHandler<fuchsia_ui_display_singleton::VsyncSource> {
 public:
  void OnVsync(
      fidl::WireEvent<fuchsia_ui_display_singleton::VsyncSource::OnVsync>* event) override {
    last_vsync_timestamp_ = zx::time(event->timestamp);
    vsync_count_++;
  }

  int vsync_count() const { return vsync_count_; }
  zx::time last_vsync_timestamp() const { return last_vsync_timestamp_; }

 private:
  int vsync_count_ = 0;
  zx::time last_vsync_timestamp_{};
};

TEST_F(VsyncSourceTest, EnableAndDisableVsync) {
  MockVsyncEventHandler mock_handler;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_ui_display_singleton::VsyncSource>::Create();
  vsync_listener_manager()->CreateBinding(std::move(server_end));
  fidl::WireClient client(std::move(client_end), dispatcher(), &mock_handler);

  // Enable vsync.
  auto result1 = client->SetVsyncEnabled(true);
  EXPECT_TRUE(result1.ok());
  RunLoopUntilIdle();

  // Trigger a vsync.
  const zx::time vsync_time = zx::time(12345);
  display_manager()->default_display()->OnVsync(zx::time_monotonic(vsync_time.get()), {});
  RunLoopUntilIdle();

  EXPECT_EQ(mock_handler.vsync_count(), 1);
  EXPECT_EQ(mock_handler.last_vsync_timestamp(), vsync_time);

  // Disable vsync.
  auto result2 = client->SetVsyncEnabled(false);
  EXPECT_TRUE(result2.ok());
  RunLoopUntilIdle();

  // Trigger another vsync.
  display_manager()->default_display()->OnVsync(zx::time_monotonic(vsync_time.get() + 1), {});
  RunLoopUntilIdle();

  // No new vsync event should be received.
  EXPECT_EQ(mock_handler.vsync_count(), 1);
}

TEST_F(VsyncSourceTest, MultipleClients) {
  MockVsyncEventHandler mock_handler1;
  auto [client_end1, server_end1] =
      fidl::Endpoints<fuchsia_ui_display_singleton::VsyncSource>::Create();
  vsync_listener_manager()->CreateBinding(std::move(server_end1));
  fidl::WireClient client1(std::move(client_end1), dispatcher(), &mock_handler1);

  MockVsyncEventHandler mock_handler2;
  auto [client_end2, server_end2] =
      fidl::Endpoints<fuchsia_ui_display_singleton::VsyncSource>::Create();
  vsync_listener_manager()->CreateBinding(std::move(server_end2));
  fidl::WireClient client2(std::move(client_end2), dispatcher(), &mock_handler2);

  // Client 1 enables vsync.
  auto result1 = client1->SetVsyncEnabled(true);
  EXPECT_TRUE(result1.ok());
  RunLoopUntilIdle();

  // Trigger a vsync.
  const zx::time vsync_time1 = zx::time(100);
  display_manager()->default_display()->OnVsync(zx::time_monotonic(vsync_time1.get()), {});
  RunLoopUntilIdle();

  EXPECT_EQ(mock_handler1.vsync_count(), 1);
  EXPECT_EQ(mock_handler1.last_vsync_timestamp(), vsync_time1);
  EXPECT_EQ(mock_handler2.vsync_count(), 0);

  // Client 2 enables vsync.
  auto result2 = client2->SetVsyncEnabled(true);
  EXPECT_TRUE(result2.ok());
  RunLoopUntilIdle();

  // Trigger another vsync.
  const zx::time vsync_time2 = zx::time(vsync_time1.get() + 100);
  display_manager()->default_display()->OnVsync(zx::time_monotonic(vsync_time2.get()), {});
  RunLoopUntilIdle();

  EXPECT_EQ(mock_handler1.vsync_count(), 2);
  EXPECT_EQ(mock_handler1.last_vsync_timestamp(), vsync_time2);
  EXPECT_EQ(mock_handler2.vsync_count(), 1);
  EXPECT_EQ(mock_handler2.last_vsync_timestamp(), vsync_time2);

  // Client 1 disables vsync.
  auto result3 = client1->SetVsyncEnabled(false);
  EXPECT_TRUE(result3.ok());
  RunLoopUntilIdle();

  // Trigger another vsync.
  const zx::time vsync_time3 = zx::time(vsync_time2.get() + 100);
  display_manager()->default_display()->OnVsync(zx::time_monotonic(vsync_time3.get()), {});
  RunLoopUntilIdle();

  EXPECT_EQ(mock_handler1.vsync_count(), 2);
  EXPECT_EQ(mock_handler2.vsync_count(), 2);
  EXPECT_EQ(mock_handler2.last_vsync_timestamp(), vsync_time3);
}

TEST_F(VsyncSourceTest, ClientDisconnects) {
  MockVsyncEventHandler mock_handler;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_ui_display_singleton::VsyncSource>::Create();
  vsync_listener_manager()->CreateBinding(std::move(server_end));
  fidl::WireClient client(std::move(client_end), dispatcher(), &mock_handler);

  auto result = client->SetVsyncEnabled(true);
  EXPECT_TRUE(result.ok());
  RunLoopUntilIdle();

  // Disconnect client.
  EXPECT_EQ(NumVsyncSourcesConnected(), 1U);
  client = {};
  RunLoopUntilIdle();
  EXPECT_EQ(NumVsyncSourcesConnected(), 0U);

  // Trigger a vsync. No crash should happen.
  display_manager()->default_display()->OnVsync(zx::time_monotonic(123), {});
  RunLoopUntilIdle();
}

}  // namespace display::test
