// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/layer.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>

#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/image-lifecycle-listener.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {

class StubImageLifecycleListener : public ImageLifecycleListener {
 public:
  using ImageWillBeDestroyedChecker = fit::function<void(display::DriverImageId)>;

  StubImageLifecycleListener() = default;
  ~StubImageLifecycleListener() = default;

  StubImageLifecycleListener(const StubImageLifecycleListener&) = delete;
  StubImageLifecycleListener& operator=(const StubImageLifecycleListener&) = delete;

  // ImageLifecycleListener:
  void ImageWillBeDestroyed(display::DriverImageId driver_image_id) override {}
};

class StubFenceListener : public FenceListener {
 public:
  StubFenceListener() = default;
  ~StubFenceListener() = default;

  StubFenceListener(const StubFenceListener&) = delete;
  StubFenceListener& operator=(const StubFenceListener&) = delete;

  // `FenceListener`:
  void OnFenceSignaled(Fence& fence) override {}
};

}  // namespace

class LayerTest : public ::testing::Test {
 public:
  LayerTest() : fences_(&fence_listener_, driver_runtime_.GetForegroundDispatcher()->borrow()) {}

  fbl::RefPtr<Image> CreateReadyImage() {
    static constexpr ClientId kClientId(1);
    static constexpr display::ImageMetadata kImageMetadata({
        .width = kDisplayWidth,
        .height = kDisplayHeight,
        .tiling_type = display::ImageTilingType::kLinear,
    });

    display::ImageId image_id = next_image_id_;
    ++next_image_id_;

    display::DriverImageId driver_image_id = next_driver_image_id_;
    ++next_driver_image_id_;

    fbl::RefPtr<Image> image = fbl::AdoptRef(new Image(
        &image_lifecycle_listener_, kImageMetadata, image_id, driver_image_id, nullptr, kClientId));
    return image;
  }

  static void MakeLayerCommitted(
      Layer& layer, fbl::DoublyLinkedList<LayerNode*>& committed_display_config_layer_list) {
    committed_display_config_layer_list.push_front(&layer.committed_display_config_list_node_);
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;

  static constexpr uint32_t kDisplayWidth = 1024;
  static constexpr uint32_t kDisplayHeight = 600;

  display::ImageId next_image_id_ = display::ImageId(1000);
  display::DriverImageId next_driver_image_id_ = display::DriverImageId(2000);

  StubImageLifecycleListener image_lifecycle_listener_;
  StubFenceListener fence_listener_;
  FenceCollection fences_;
};

TEST_F(LayerTest, PrimaryBasic) {
  Layer layer(display::LayerId(1));

  const display::ImageMetadata image_metadata({.width = kDisplayWidth,
                                               .height = kDisplayHeight,
                                               .tiling_type = display::ImageTilingType::kLinear});
  const display::Rectangle display_area(
      {.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight});
  layer.SetPrimaryConfig(image_metadata);
  layer.SetPrimaryPosition(display::CoordinateTransformation::kIdentity, display_area,
                           display_area);
  layer.SetPrimaryAlpha(display::AlphaMode::kDisable, 0);
  fbl::RefPtr<Image> image = CreateReadyImage();
  layer.SetImage(image, display::kInvalidEventId);
  layer.CommitChanges();
}

TEST_F(LayerTest, CleanUpImage) {
  Layer layer(display::LayerId(1));

  const display::ImageMetadata image_metadata({.width = kDisplayWidth,
                                               .height = kDisplayHeight,
                                               .tiling_type = display::ImageTilingType::kLinear});
  const display::Rectangle display_area(
      {.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight});
  layer.SetPrimaryConfig(image_metadata);
  layer.SetPrimaryPosition(display::CoordinateTransformation::kIdentity, display_area,
                           display_area);
  layer.SetPrimaryAlpha(display::AlphaMode::kDisable, 0);

  auto displayed_image = CreateReadyImage();
  layer.SetImage(displayed_image, display::kInvalidEventId);
  layer.CommitChanges();

  ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(1)));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  constexpr display::EventId kWaitFenceId(1);
  ASSERT_OK(fences_.ImportEvent(std::move(event), kWaitFenceId));
  auto fence_release = fit::defer([&] { fences_.ReleaseEvent(kWaitFenceId); });

  auto waiting_image = CreateReadyImage();
  layer.SetImage(waiting_image, kWaitFenceId);
  ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(2)));

  auto draft_image = CreateReadyImage();
  layer.SetImage(draft_image, display::kInvalidEventId);

  ASSERT_TRUE(layer.ActivateLatestReadyImage());

  EXPECT_TRUE(layer.committed_image());

  // Nothing should happen if image doesn't match.
  auto not_matching_image = CreateReadyImage();
  EXPECT_FALSE(layer.CleanUpImage(*not_matching_image));
  EXPECT_TRUE(layer.committed_image());

  // Test cleaning up a waiting image.
  EXPECT_FALSE(layer.CleanUpImage(*waiting_image));
  EXPECT_TRUE(layer.committed_image());

  // Test cleaning up a draft image.
  EXPECT_FALSE(layer.CleanUpImage(*draft_image));
  EXPECT_TRUE(layer.committed_image());

  // Test cleaning up the associated image.
  //
  // The layer is not in a display's committed configuration list. So, cleaning up
  // the layer's image doesn't change the committed config.
  EXPECT_FALSE(layer.CleanUpImage(*displayed_image));
  EXPECT_FALSE(layer.committed_image());
}

TEST_F(LayerTest, CleanUpImage_CheckConfigChange) {
  fbl::DoublyLinkedList<LayerNode*> applied_layers;

  Layer layer(display::LayerId(1));

  const display::ImageMetadata image_metadata({.width = kDisplayWidth,
                                               .height = kDisplayHeight,
                                               .tiling_type = display::ImageTilingType::kLinear});
  const display::Rectangle display_area(
      {.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight});
  layer.SetPrimaryConfig(image_metadata);
  layer.SetPrimaryPosition(display::CoordinateTransformation::kIdentity, display_area,
                           display_area);
  layer.SetPrimaryAlpha(display::AlphaMode::kDisable, 0);

  // Clean up images, which doesn't change the committed config.
  {
    fbl::RefPtr<Image> image = CreateReadyImage();
    layer.SetImage(image, display::kInvalidEventId);
    layer.CommitChanges();
    ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(1)));
    ASSERT_TRUE(layer.ActivateLatestReadyImage());

    EXPECT_TRUE(layer.committed_image());
    // The layer is not in a display's committed configuration list. So, cleaning
    // up the layer's image doesn't change the committed config.
    EXPECT_FALSE(layer.CleanUpImage(*image));
    EXPECT_FALSE(layer.committed_image());
  }

  // Clean up images, which changes the committed config.
  {
    MakeLayerCommitted(layer, applied_layers);

    fbl::RefPtr<Image> image = CreateReadyImage();
    layer.SetImage(image, display::kInvalidEventId);
    layer.CommitChanges();
    ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(2)));
    ASSERT_TRUE(layer.ActivateLatestReadyImage());

    EXPECT_TRUE(layer.committed_image());

    // The layer is in a display's committed configuration list. So, cleaning up
    // the layer's image changes the committed config.
    EXPECT_TRUE(layer.CleanUpImage(*image));
    EXPECT_FALSE(layer.committed_image());

    applied_layers.clear();
  }
}

TEST_F(LayerTest, CleanUpAllImages) {
  Layer layer(display::LayerId(1));

  const display::ImageMetadata image_metadata({.width = kDisplayWidth,
                                               .height = kDisplayHeight,
                                               .tiling_type = display::ImageTilingType::kLinear});
  const display::Rectangle display_area(
      {.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight});
  layer.SetPrimaryConfig(image_metadata);
  layer.SetPrimaryPosition(display::CoordinateTransformation::kIdentity, display_area,
                           display_area);
  layer.SetPrimaryAlpha(display::AlphaMode::kDisable, 0);

  auto displayed_image = CreateReadyImage();
  layer.SetImage(displayed_image, display::kInvalidEventId);
  layer.CommitChanges();
  ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(1)));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  constexpr display::EventId kWaitFenceId(1);
  ASSERT_OK(fences_.ImportEvent(std::move(event), kWaitFenceId));
  auto fence_release = fit::defer([&] { fences_.ReleaseEvent(kWaitFenceId); });

  auto waiting_image = CreateReadyImage();
  layer.SetImage(waiting_image, kWaitFenceId);
  ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(2)));

  auto draft_image = CreateReadyImage();
  layer.SetImage(draft_image, display::kInvalidEventId);

  ASSERT_TRUE(layer.ActivateLatestReadyImage());

  // The layer is not in a display's committed configuration list. So, cleaning
  // up the layer's image doesn't change the committed config.
  EXPECT_FALSE(layer.CleanUpAllImages());
  EXPECT_FALSE(layer.committed_image());
}

TEST_F(LayerTest, CleanUpAllImages_CheckConfigChange) {
  fbl::DoublyLinkedList<LayerNode*> applied_layers;

  Layer layer(display::LayerId(1));

  const display::ImageMetadata image_metadata({.width = kDisplayWidth,
                                               .height = kDisplayHeight,
                                               .tiling_type = display::ImageTilingType::kLinear});
  const display::Rectangle display_area(
      {.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight});
  layer.SetPrimaryConfig(image_metadata);
  layer.SetPrimaryPosition(display::CoordinateTransformation::kIdentity, display_area,
                           display_area);
  layer.SetPrimaryAlpha(display::AlphaMode::kDisable, 0);

  // Clean up all images, which doesn't change the committed config.
  {
    fbl::RefPtr<Image> image = CreateReadyImage();
    layer.SetImage(image, display::kInvalidEventId);
    layer.CommitChanges();
    ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(1)));
    ASSERT_TRUE(layer.ActivateLatestReadyImage());

    EXPECT_TRUE(layer.committed_image());
    // The layer is not in a display's committed configuration list. So, cleaning
    // up the layer's image doesn't change the committed config.
    EXPECT_FALSE(layer.CleanUpAllImages());
    EXPECT_FALSE(layer.committed_image());
  }

  // Clean up all images, which changes the committed config.
  {
    MakeLayerCommitted(layer, applied_layers);

    fbl::RefPtr<Image> image = CreateReadyImage();
    layer.SetImage(image, display::kInvalidEventId);
    layer.CommitChanges();
    ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(2)));
    ASSERT_TRUE(layer.ActivateLatestReadyImage());

    EXPECT_TRUE(layer.committed_image());
    // The layer is in a display's committed configuration list. So, cleaning up
    // the layer's image changes the committed config.
    EXPECT_TRUE(layer.CleanUpAllImages());
    EXPECT_FALSE(layer.committed_image());

    applied_layers.clear();
  }
}

TEST_F(LayerTest, ReuseImageInDifferentConfigs) {
  Layer layer(display::LayerId(1));

  const display::ImageMetadata image_metadata({.width = kDisplayWidth,
                                               .height = kDisplayHeight,
                                               .tiling_type = display::ImageTilingType::kLinear});
  const display::Rectangle display_area(
      {.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight});
  layer.SetPrimaryConfig(image_metadata);
  layer.SetPrimaryPosition(display::CoordinateTransformation::kIdentity, display_area,
                           display_area);
  layer.SetPrimaryAlpha(display::AlphaMode::kDisable, 0);

  fbl::RefPtr<Image> image = CreateReadyImage();
  layer.SetImage(image, display::kInvalidEventId);
  layer.CommitChanges();
  ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(1)));
  ASSERT_TRUE(layer.ActivateLatestReadyImage());

  EXPECT_TRUE(layer.committed_image());
  EXPECT_EQ(layer.committed_image()->id(), image->id());

  layer.SetImage(image, display::kInvalidEventId);
  layer.CommitChanges();
  ASSERT_TRUE(layer.ResolveDraftImage(&fences_, display::ConfigStamp(2)));
  ASSERT_TRUE(layer.ActivateLatestReadyImage());

  EXPECT_TRUE(layer.committed_image());
  EXPECT_EQ(layer.committed_image()->id(), image->id());
}

}  // namespace display_coordinator
