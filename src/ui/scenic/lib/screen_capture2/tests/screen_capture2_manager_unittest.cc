// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screen_capture2/screen_capture2_manager.h"

#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>

#include <memory>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/flatland/engine/engine.h"
#include "src/ui/scenic/lib/flatland/renderer/null_renderer.h"
#include "src/ui/scenic/lib/flatland/tests/mock_flatland_presenter.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_buffer_collection_importer.h"
#include "src/ui/scenic/lib/screen_capture2/tests/common.h"
#include "src/ui/scenic/lib/utils/helpers.h"

using testing::_;

using allocation::Allocator;
using flatland::SrcToDest;
using fuchsia_ui_composition_internal::FrameInfo;
using fuchsia_ui_composition_internal::ScreenCaptureConfig;
using fuchsia_ui_composition_internal::ScreenCaptureError;
using screen_capture::ScreenCaptureBufferCollectionImporter;

namespace screen_capture2 {
namespace test {

class ScreenCapture2ManagerTest : public gtest::TestLoopFixture {
 public:
  ScreenCapture2ManagerTest() = default;

  void RunLoopUntil(fit::function<bool()> condition) {
    while (!condition()) {
      RunLoopUntilIdle();
    }
  }

  void SetUp() override {
    // Create the SysmemAllocator.
    sysmem_allocator_ =
        utils::CreateSysmemAllocatorClient(dispatcher(), "ScreenCapture2ManagerTest");

    renderer_ = std::make_shared<flatland::NullRenderer>();
    importer_ = std::make_unique<ScreenCaptureBufferCollectionImporter>(
        utils::CreateSysmemAllocatorClient(dispatcher(), "ScreenCapture2ManagerTest-importer"),
        renderer_);

    manager_ = std::make_unique<ScreenCapture2Manager>(
        renderer_, importer_, std::bind(&ScreenCapture2ManagerTest::GetRenderables, this));
  }

  void TearDown() override {
    manager_.reset();
    RunLoopUntilIdle();
  }

  fidl::Client<fuchsia_ui_composition_internal::ScreenCapture> CreateScreenCapture(
      fidl::AsyncEventHandler<fuchsia_ui_composition_internal::ScreenCapture>* event_handler =
          nullptr) {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_ui_composition_internal::ScreenCapture>::Create();
    manager_->CreateClient(std::move(server_end));
    return fidl::Client<fuchsia_ui_composition_internal::ScreenCapture>(
        std::move(client_end), dispatcher(), event_handler);
  }

  flatland::Renderables GetRenderables() { return {}; }

  void ConfigureScreenCapture(fidl::Client<fuchsia_ui_composition_internal::ScreenCapture>& sc,
                              BufferCount buffer_count, uint32_t image_width,
                              uint32_t image_height) {
    allocation::cpp::BufferCollectionImportExportTokens ref_pair =
        allocation::cpp::BufferCollectionImportExportTokens::New();

    std::shared_ptr<Allocator> flatland_allocator =
        CreateAllocator(importer_, context_provider_.context(), dispatcher());
    CreateBufferCollectionInfoWithConstraints(
        utils::CreateDefaultConstraints(buffer_count, image_width, image_height),
        std::move(ref_pair.export_token), flatland_allocator, sysmem_allocator_, dispatcher(),
        [this](fit::function<bool()> condition) { RunLoopUntil(std::move(condition)); });

    fuchsia_ui_composition_internal::ScreenCaptureConfig args;
    args.import_token(std::move(ref_pair.import_token));
    args.image_size(fuchsia_math::SizeU{image_width, image_height});

    bool configure_succeeded = false;
    sc->Configure(std::move(args)).Then([&configure_succeeded](auto& result) {
      ASSERT_TRUE(result.is_ok());
      configure_succeeded = true;
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(configure_succeeded);
  }

 protected:
  std::shared_ptr<flatland::Renderer> renderer_;
  std::shared_ptr<screen_capture::ScreenCaptureBufferCollectionImporter> importer_;
  std::unique_ptr<ScreenCapture2Manager> manager_;

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  sys::testing::ComponentContextProvider context_provider_;
};

TEST_F(ScreenCapture2ManagerTest, CreateClients) {
  fidl::Client sc1 = CreateScreenCapture();
  fidl::Client sc2 = CreateScreenCapture();

  RunLoopUntilIdle();
  EXPECT_TRUE(sc1.is_valid());
  EXPECT_TRUE(sc2.is_valid());

  EXPECT_EQ(manager_->client_count(), 2ul);
}

TEST_F(ScreenCapture2ManagerTest, ClientDiesBeforeManager) {
  {
    fidl::Client sc = CreateScreenCapture();
    RunLoopUntilIdle();
    EXPECT_TRUE(sc.is_valid());
    EXPECT_EQ(manager_->client_count(), 1ul);
    // |sc| falls out of scope.
  }
  RunLoopUntilIdle();

  EXPECT_EQ(manager_->client_count(), 0ul);
}

TEST_F(ScreenCapture2ManagerTest, ManagerDiesBeforeClients) {
  class EventHandler
      : public fidl::AsyncEventHandler<fuchsia_ui_composition_internal::ScreenCapture> {
   public:
    void on_fidl_error(fidl::UnbindInfo info) override { error_called_ = true; }
    bool error_called_ = false;
  };

  EventHandler handler1;
  EventHandler handler2;
  fidl::Client sc1 = CreateScreenCapture(&handler1);
  fidl::Client sc2 = CreateScreenCapture(&handler2);

  RunLoopUntilIdle();
  EXPECT_TRUE(sc1.is_valid());
  EXPECT_TRUE(sc2.is_valid());

  EXPECT_EQ(manager_->client_count(), 2ul);

  manager_.reset();
  RunLoopUntilIdle();
  EXPECT_TRUE(handler1.error_called_);
  EXPECT_TRUE(handler2.error_called_);
}

TEST_F(ScreenCapture2ManagerTest, Client_Configure) {
  fidl::Client sc = CreateScreenCapture();
  RunLoopUntilIdle();
  EXPECT_TRUE(sc.is_valid());
  EXPECT_EQ(manager_->client_count(), 1ul);

  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  ConfigureScreenCapture(sc, buffer_count, image_width, image_height);
}

TEST_F(ScreenCapture2ManagerTest, Manager_RenderPendingScreenCaptures) {
  fidl::Client sc = CreateScreenCapture();
  RunLoopUntilIdle();
  EXPECT_TRUE(sc.is_valid());
  EXPECT_EQ(manager_->client_count(), 1ul);

  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  ConfigureScreenCapture(sc, buffer_count, image_width, image_height);

  FrameInfo info;
  bool first_get_next_frame_succeeded = false;
  sc->GetNextFrame().Then([&first_get_next_frame_succeeded, &info](auto& result) {
    ASSERT_TRUE(result.is_ok());
    info = std::move(result.value());
    first_get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();
  EXPECT_TRUE(first_get_next_frame_succeeded);

  zx::eventpair token = std::move(info.buffer_release_token().value());
  EXPECT_EQ(token.signal_peer(0, ZX_EVENTPAIR_SIGNALED), ZX_OK);
  RunLoopUntilIdle();

  FrameInfo info2;
  bool second_get_next_frame_succeeded = false;
  sc->GetNextFrame().Then([&second_get_next_frame_succeeded, &info2](auto& result) {
    ASSERT_TRUE(result.is_ok());
    info2 = std::move(result.value());
    second_get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();
  EXPECT_FALSE(second_get_next_frame_succeeded);

  // Since |received_last_frame_| is true, GetNextFrame() will be hanging for new frame.
  manager_->RenderPendingScreenCaptures();
  RunLoopUntilIdle();

  EXPECT_TRUE(second_get_next_frame_succeeded);
  EXPECT_EQ(info2.buffer_index().value(), info.buffer_index().value());
}

// Expects |render_frame_in_use_| to lock MaybeRenderFrame() and Client to receive expected frame.
TEST_F(ScreenCapture2ManagerTest, ManagerClient_BothWantNewFrame) {
  fidl::Client sc = CreateScreenCapture();
  RunLoopUntilIdle();
  EXPECT_TRUE(sc.is_valid());
  EXPECT_EQ(manager_->client_count(), 1ul);

  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  ConfigureScreenCapture(sc, buffer_count, image_width, image_height);

  bool get_next_frame_succeeded = false;
  sc->GetNextFrame().Then([&get_next_frame_succeeded](auto& result) {
    ASSERT_TRUE(result.is_ok());
    get_next_frame_succeeded = true;
  });
  manager_->RenderPendingScreenCaptures();
  RunLoopUntilIdle();
  EXPECT_TRUE(get_next_frame_succeeded);
}

}  // namespace test
}  // namespace screen_capture2
