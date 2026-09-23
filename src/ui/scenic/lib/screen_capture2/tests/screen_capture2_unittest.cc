// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screen_capture2/screen_capture2.h"

#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fpromise/promise.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>

#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/allocation/mock_buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/buffers/buffer_collection.h"
#include "src/ui/scenic/lib/flatland/renderer/mock_renderer.h"
#include "src/ui/scenic/lib/flatland/renderer/null_renderer.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_buffer_collection_importer.h"
#include "src/ui/scenic/lib/screen_capture2/tests/common.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"

using allocation::Allocator;
using allocation::BufferCollectionImporter;
using flatland::SrcToDest;
using fuchsia_ui_composition_internal::FrameInfo;
using fuchsia_ui_composition_internal::ScreenCaptureConfig;
using fuchsia_ui_composition_internal::ScreenCaptureError;
using integration_tests::ReturnPromise;
using screen_capture::ScreenCaptureBufferCollectionImporter;
using testing::_;

namespace screen_capture2 {
namespace test {

class ScreenCapture2Test : public gtest::TestLoopFixture {
 public:
  ScreenCapture2Test() = default;

  void RunLoopUntil(fit::function<bool()> condition) {
    while (!condition()) {
      RunLoopUntilIdle();
    }
  }

  void SetUp() override {
    // Create the SysmemAllocator.
    sysmem_allocator_ = utils::CreateSysmemAllocatorClient(dispatcher(), "ScreenCapture2Test");

    renderer_ = std::make_shared<flatland::NullRenderer>();
    importer_ = std::make_shared<ScreenCaptureBufferCollectionImporter>(
        utils::CreateSysmemAllocatorClient(dispatcher(), "ScreenCapture2Test-importer"), renderer_);

    renderables_ = {};
  }

  void SetUpMockImporter() {
    mock_renderer_ = std::make_shared<flatland::MockRenderer>();
    importer_ = std::make_shared<ScreenCaptureBufferCollectionImporter>(
        utils::CreateSysmemAllocatorClient(dispatcher(), "ScreenCapture2Test-mock-importer"),
        mock_renderer_);
  }

  fidl::Client<fuchsia_ui_composition_internal::ScreenCapture> BindScreenCapture(
      screen_capture2::ScreenCapture& sc_server) {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_ui_composition_internal::ScreenCapture>::Create();
    fidl::BindServer(dispatcher(), std::move(server_end), &sc_server);
    return fidl::Client<fuchsia_ui_composition_internal::ScreenCapture>(std::move(client_end),
                                                                        dispatcher());
  }

  // Configures ScreenCapture with given args successfully.
  void SetUpScreenCapture(fidl::Client<fuchsia_ui_composition_internal::ScreenCapture>& sc_client,
                          BufferCount buffer_count, uint32_t image_width, uint32_t image_height,
                          bool is_mock) {
    if (is_mock) {
      EXPECT_CALL(*mock_renderer_.get(), ImportBufferCollection(_, _, _, _, _))
          .WillRepeatedly([](allocation::GlobalBufferCollectionId collection_id,
                             fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                             fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
                             allocation::BufferCollectionUsage,
                             std::optional<fuchsia_math::SizeU> size) {
            auto result = flatland::BufferCollectionInfo::New(sysmem_allocator, std::move(token));
            if (result.is_error()) {
              FX_LOGS(WARNING) << "Unable to register collection.";
              return fpromise::make_error_promise();
            }
            return fpromise::make_ok_promise();
          });

      EXPECT_CALL(*mock_renderer_.get(), ImportBufferImage(_, _))
          .WillRepeatedly(ReturnPromise(fpromise::ok()));

      EXPECT_CALL(*mock_renderer_.get(), Render(_, _, _))
          .WillRepeatedly([](const allocation::ImageMetadata& render_target,
                             std::span<const flatland::ResolvedLayer> layers,
                             const flatland::Renderer::RenderArgs& render_args) {
            // Fire all of the release fences.
            for (auto& fence : render_args.release_fences) {
              fence.signal(0, ZX_EVENT_SIGNALED);
            }
          });

      EXPECT_CALL(*mock_renderer_.get(), ReleaseBufferCollection(_, _))
          .Times(::testing::AtLeast(0));
    }

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
    sc_client->Configure(std::move(args)).Then([&configure_succeeded](auto& result) {
      ASSERT_TRUE(result.is_ok());
      configure_succeeded = true;
    });
    RunLoopUntilIdle();
    ASSERT_TRUE(configure_succeeded);
  }

  std::vector<flatland::ResolvedLayer> GetRenderables() { return renderables_; }

  bool GetReceivedLastFrame(screen_capture2::ScreenCapture& sc_server) {
    return sc_server.get_client_received_last_frame();
  }

 protected:
  std::shared_ptr<flatland::NullRenderer> renderer_;
  std::shared_ptr<flatland::MockRenderer> mock_renderer_;
  std::shared_ptr<ScreenCaptureBufferCollectionImporter> importer_;

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  sys::testing::ComponentContextProvider context_provider_;

 private:
  std::vector<flatland::ResolvedLayer> renderables_;
};

TEST_F(ScreenCapture2Test, ConfigureWithMissingArguments) {
  screen_capture2::ScreenCapture sc_server(importer_, nullptr,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);

  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;
  allocation::cpp::BufferCollectionImportExportTokens ref_pair =
      allocation::cpp::BufferCollectionImportExportTokens::New();

  std::shared_ptr<Allocator> flatland_allocator =
      CreateAllocator(importer_, context_provider_.context(), dispatcher());
  CreateBufferCollectionInfoWithConstraints(
      utils::CreateDefaultConstraints(buffer_count, image_width, image_height),
      std::move(ref_pair.export_token), flatland_allocator, sysmem_allocator_, dispatcher(),
      [this](fit::function<bool()> condition) { RunLoopUntil(std::move(condition)); });

  // Missing image size.
  {
    fuchsia_ui_composition_internal::ScreenCaptureConfig args;
    args.import_token(std::move(ref_pair.import_token));

    bool called = false;
    sc_client->Configure(std::move(args)).Then([&called](auto& result) {
      ASSERT_TRUE(result.is_error());
      ASSERT_TRUE(result.error_value().is_domain_error());
      EXPECT_EQ(result.error_value().domain_error(), ScreenCaptureError::kMissingArgs);
      called = true;
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(called);
  }

  // Missing import token.
  {
    fuchsia_ui_composition_internal::ScreenCaptureConfig args;
    args.image_size(fuchsia_math::SizeU{image_width, image_height});

    bool called = false;
    sc_client->Configure(std::move(args)).Then([&called](auto& result) {
      ASSERT_TRUE(result.is_error());
      ASSERT_TRUE(result.error_value().is_domain_error());
      EXPECT_EQ(result.error_value().domain_error(), ScreenCaptureError::kMissingArgs);
      called = true;
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(called);
  }

  // Unable to get buffer count.
  {
    allocation::cpp::BufferCollectionImportExportTokens ref_pair2 =
        allocation::cpp::BufferCollectionImportExportTokens::New();

    fuchsia_ui_composition_internal::ScreenCaptureConfig args;
    args.import_token(std::move(ref_pair2.import_token));
    args.image_size(fuchsia_math::SizeU{image_width, image_height});

    bool called = false;
    sc_client->Configure(std::move(args)).Then([&called](auto& result) {
      ASSERT_TRUE(result.is_error());
      ASSERT_TRUE(result.error_value().is_domain_error());
      EXPECT_EQ(result.error_value().domain_error(), ScreenCaptureError::kInvalidArgs);
      called = true;
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(called);
  }

  // Has invalid import token.
  {
    allocation::cpp::BufferCollectionImportExportTokens ref_pair2 =
        allocation::cpp::BufferCollectionImportExportTokens::New();
    ref_pair2.import_token.value().reset();

    fuchsia_ui_composition_internal::ScreenCaptureConfig args;
    args.import_token(std::move(ref_pair2.import_token));
    args.image_size(fuchsia_math::SizeU{image_width, image_height});

    bool called = false;
    sc_client->Configure(std::move(args)).Then([&called](auto& result) {
      ASSERT_TRUE(result.is_error());
      called = true;
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(called);
  }
}

// The test uses a mock to fail ImportBufferImage() at a specific buffer during Configure()
// and ensure ReleaseBufferImage gets called the correct number of times.
TEST_F(ScreenCapture2Test, Configure_BufferCollectionFailure) {
  SetUpMockImporter();
  EXPECT_CALL(*mock_renderer_.get(), ImportBufferCollection(_, _, _, _, _))
      .WillRepeatedly([](allocation::GlobalBufferCollectionId collection_id,
                         fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                         fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
                         allocation::BufferCollectionUsage,
                         std::optional<fuchsia_math::SizeU> size) {
        auto result = flatland::BufferCollectionInfo::New(sysmem_allocator, std::move(token));
        if (result.is_error()) {
          FX_LOGS(WARNING) << "Unable to register collection.";
          return fpromise::make_error_promise();
        }
        return fpromise::make_ok_promise();
      });

  screen_capture2::ScreenCapture sc_server(importer_, nullptr,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);

  const BufferCount buffer_count = 3;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;
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

  EXPECT_CALL(*mock_renderer_.get(), ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()))
      .WillOnce(ReturnPromise(fpromise::ok()))
      .WillOnce(ReturnPromise(fpromise::error()));

  EXPECT_CALL(*mock_renderer_.get(), ReleaseBufferImage(_)).Times(buffer_count - 1);

  bool called = false;
  sc_client->Configure(std::move(args)).Then([&called](auto& result) {
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    EXPECT_EQ(result.error_value().domain_error(), ScreenCaptureError::kInvalidArgs);
    called = true;
  });
  RunLoopUntilIdle();
  EXPECT_TRUE(called);

  // Expect all BufferImages are released before any tear down of test.
  EXPECT_CALL(*mock_renderer_, ReleaseBufferImage(_)).Times(0);
}

TEST_F(ScreenCapture2Test, Configure_Success) {
  screen_capture2::ScreenCapture sc_server(importer_, renderer_,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);
  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  SetUpScreenCapture(sc_client, buffer_count, image_width, image_height, false);
}

TEST_F(ScreenCapture2Test, GetNextFrame_Success) {
  screen_capture2::ScreenCapture sc_server(importer_, renderer_,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);
  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  SetUpScreenCapture(sc_client, buffer_count, image_width, image_height, false);

  bool get_next_frame_succeeded = false;
  sc_client->GetNextFrame().Then([&get_next_frame_succeeded](auto& result) {
    ASSERT_TRUE(result.is_ok());
    get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();
  EXPECT_TRUE(get_next_frame_succeeded);
}

// Releasing buffer after first render allows it to be reused during successive call.
TEST_F(ScreenCapture2Test, GetNextFrame_SuccessiveCallSuccess) {
  SetUpMockImporter();
  screen_capture2::ScreenCapture sc_server(importer_, mock_renderer_,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);
  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  SetUpScreenCapture(sc_client, buffer_count, image_width, image_height, true);

  fuchsia_ui_composition_internal::FrameInfo info;
  bool first_get_next_frame_succeeded = false;
  sc_client->GetNextFrame().Then([&first_get_next_frame_succeeded, &info](auto& result) {
    ASSERT_TRUE(result.is_ok());
    info = std::move(result.value());
    first_get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();
  EXPECT_TRUE(first_get_next_frame_succeeded);

  EXPECT_TRUE(GetReceivedLastFrame(sc_server));

  zx::eventpair token = std::move(info.buffer_release_token().value());
  EXPECT_EQ(token.signal_peer(0, ZX_EVENTPAIR_SIGNALED), ZX_OK);
  RunLoopUntilIdle();

  fuchsia_ui_composition_internal::FrameInfo info2;
  bool second_get_next_frame_succeeded = false;
  sc_client->GetNextFrame().Then([&second_get_next_frame_succeeded, &info2](auto& result) {
    ASSERT_TRUE(result.is_ok());
    info2 = std::move(result.value());
    second_get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();

  // Since |received_last_frame_| is true, GetNextFrame() will be hanging.
  sc_server.MaybeRenderFrame();
  RunLoopUntilIdle();

  EXPECT_TRUE(second_get_next_frame_succeeded);
  EXPECT_EQ(info2.buffer_index().value(), info.buffer_index().value());

  EXPECT_CALL(*mock_renderer_, ReleaseBufferImage(_)).Times(1);
}

TEST_F(ScreenCapture2Test, GetNextFrame_Errors) {
  SetUpMockImporter();
  screen_capture2::ScreenCapture sc_server(importer_, mock_renderer_,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);
  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  SetUpScreenCapture(sc_client, buffer_count, image_width, image_height, true);

  // Overwriting hanging get.
  {
    // Capture the renderer's release fence instead of immediately signaling it (the default mock
    // behavior in `SetUpScreenCapture()`). This avoids a race by guaranteeing that the first
    // `GetNextFrame()` is still pending on the server when the second request is dispatched, which
    // is the condition that triggers a `kBadHangingGet` error. Without this, the reply to the
    // first call and dispatch of the second would race: `TestLoop` randomizes the dispatch order of
    // concurrently-ready waits, so on some seeds the second call would become a legal hanging get
    // and the test would fail.
    zx::event release_fence;
    EXPECT_CALL(*mock_renderer_.get(), Render(_, _, _))
        .WillOnce([&release_fence](const allocation::ImageMetadata& render_target,
                                   std::span<const flatland::ResolvedLayer> layers,
                                   const flatland::Renderer::RenderArgs& render_args) {
          ASSERT_FALSE(render_args.release_fences.empty());
          render_args.release_fences[0].duplicate(ZX_RIGHT_SAME_RIGHTS, &release_fence);
        });

    bool first_get_next_frame_succeeded = false;
    sc_client->GetNextFrame().Then([&first_get_next_frame_succeeded](auto& result) {
      ASSERT_TRUE(result.is_ok());
      first_get_next_frame_succeeded = true;
    });
    bool second_get_next_frame_succeeded = false;
    sc_client->GetNextFrame().Then([&second_get_next_frame_succeeded](auto& result) {
      ASSERT_TRUE(result.is_error());
      ASSERT_TRUE(result.error_value().is_domain_error());
      EXPECT_EQ(result.error_value().domain_error(), ScreenCaptureError::kBadHangingGet);
      second_get_next_frame_succeeded = true;
    });
    RunLoopUntilIdle();

    // The second request failed immediately with `kBadHangingGet`, and the first request hasn't
    // finished because it's waiting for the release fence to be signaled.
    EXPECT_FALSE(first_get_next_frame_succeeded);
    EXPECT_TRUE(second_get_next_frame_succeeded);

    // Signal the release fence to allow the first GetNextFrame to complete.
    ASSERT_TRUE(release_fence.is_valid());
    release_fence.signal(0, ZX_EVENT_SIGNALED);
    RunLoopUntilIdle();
    EXPECT_TRUE(first_get_next_frame_succeeded);

    EXPECT_CALL(*mock_renderer_, ReleaseBufferImage(_)).Times(1);
  }
}

// Releasing buffer while client has been waiting immediately renders the frame.
TEST_F(ScreenCapture2Test, GetNextFrame_BuffersFull) {
  SetUpMockImporter();
  screen_capture2::ScreenCapture sc_server(importer_, mock_renderer_,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);

  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  SetUpScreenCapture(sc_client, buffer_count, image_width, image_height, true);

  // Makes buffer unavailable.
  fuchsia_ui_composition_internal::FrameInfo info;
  bool first_get_next_frame_succeeded = false;
  sc_client->GetNextFrame().Then([&first_get_next_frame_succeeded, &info](auto& result) {
    ASSERT_TRUE(result.is_ok());
    info = std::move(result.value());
    first_get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();
  EXPECT_TRUE(first_get_next_frame_succeeded);

  zx::eventpair token = std::move(info.buffer_release_token().value());

  bool second_get_next_frame_succeeded = false;
  fuchsia_ui_composition_internal::FrameInfo info2;
  sc_client->GetNextFrame().Then([&second_get_next_frame_succeeded, &info2](auto& result) {
    ASSERT_TRUE(result.is_ok());
    info2 = std::move(result.value());
    second_get_next_frame_succeeded = true;
  });
  RunLoopUntilIdle();
  EXPECT_FALSE(second_get_next_frame_succeeded);

  EXPECT_EQ(token.signal_peer(0, ZX_EVENTPAIR_SIGNALED), ZX_OK);
  RunLoopUntilIdle();

  // Since |received_last_frame_| is true, GetNextFrame will be hanging.
  sc_server.MaybeRenderFrame();
  RunLoopUntilIdle();

  EXPECT_TRUE(second_get_next_frame_succeeded);
  EXPECT_EQ(info2.buffer_index().value(), info.buffer_index().value());

  EXPECT_CALL(*mock_renderer_, ReleaseBufferImage(_)).Times(1);
}

TEST_F(ScreenCapture2Test, MaybeRenderFrame_Errors) {
  SetUpMockImporter();
  screen_capture2::ScreenCapture sc_server(importer_, mock_renderer_,
                                           [this]() { return this->GetRenderables(); });
  fidl::Client sc_client = BindScreenCapture(sc_server);

  const BufferCount buffer_count = 1;
  const uint32_t image_width = 1;
  const uint32_t image_height = 1;

  SetUpScreenCapture(sc_client, buffer_count, image_width, image_height, true);

  // |available_buffers_| is empty.
  {
    fuchsia_ui_composition_internal::FrameInfo info;
    bool first_get_next_frame_succeeded = false;
    sc_client->GetNextFrame().Then([&first_get_next_frame_succeeded, &info](auto& result) {
      ASSERT_TRUE(result.is_ok());
      info = std::move(result.value());
      first_get_next_frame_succeeded = true;
    });
    RunLoopUntilIdle();
    EXPECT_TRUE(first_get_next_frame_succeeded);
    EXPECT_TRUE(GetReceivedLastFrame(sc_server));

    bool second_get_next_frame_succeeded = false;
    sc_client->GetNextFrame().Then([&second_get_next_frame_succeeded](auto& result) {
      ASSERT_TRUE(result.is_ok());
      second_get_next_frame_succeeded = true;
    });
    RunLoopUntilIdle();
    sc_server.MaybeRenderFrame();
    RunLoopUntilIdle();
    EXPECT_FALSE(second_get_next_frame_succeeded);
    EXPECT_FALSE(GetReceivedLastFrame(sc_server));

    // Clean up test.
    zx::eventpair token = std::move(info.buffer_release_token().value());
    EXPECT_EQ(token.signal_peer(0, ZX_EVENTPAIR_SIGNALED), ZX_OK);
    RunLoopUntilIdle();

    EXPECT_TRUE(GetReceivedLastFrame(sc_server));
  }

  // |current_completer_| does not exist.
  {
    EXPECT_TRUE(GetReceivedLastFrame(sc_server));

    sc_server.MaybeRenderFrame();
    RunLoopUntilIdle();
    EXPECT_FALSE(GetReceivedLastFrame(sc_server));
  }
}

}  // namespace test
}  // namespace screen_capture2
