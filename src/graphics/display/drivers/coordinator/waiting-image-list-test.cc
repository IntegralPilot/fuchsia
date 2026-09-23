// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/waiting-image-list.h"

#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/defer.h>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/image-lifecycle-listener.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {

class StubImageLifecycleListener : public ImageLifecycleListener {
 public:
  StubImageLifecycleListener() = default;
  ~StubImageLifecycleListener() = default;

  StubImageLifecycleListener(const StubImageLifecycleListener&) = delete;
  StubImageLifecycleListener& operator=(const StubImageLifecycleListener&) = delete;

  // ImageLifecycleListener:
  void ImageWillBeDestroyed(display::DriverImageId driver_image_id) override {}
};

class FakeFenceListener : public FenceListener {
 public:
  // `waiting_images` must not be null and must outlive this instance.
  explicit FakeFenceListener(WaitingImageList* waiting_images) : waiting_images_(*waiting_images) {
    ZX_DEBUG_ASSERT(waiting_images != nullptr);
  }

  FakeFenceListener(const FakeFenceListener&) = delete;
  FakeFenceListener& operator=(const FakeFenceListener&) = delete;

  ~FakeFenceListener() override = default;

  // `FenceListener`:
  void OnFenceSignaled(Fence& fence) override { waiting_images_.MarkFenceReady(fence); }

 private:
  WaitingImageList& waiting_images_;
};

class WaitingImageListTest : public ::testing::Test {
 public:
  WaitingImageListTest()
      : fences_(&fence_listener_, driver_runtime_.GetForegroundDispatcher()->borrow()) {}

  ~WaitingImageListTest() override = default;

  fbl::RefPtr<Image> CreateImage() {
    static constexpr ClientId kClientId(1);
    static constexpr display::ImageMetadata kImageMetadata({
        .width = 100,
        .height = 200,
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

  WaitingImageList& waiting_images() { return waiting_images_; }
  FenceCollection& fences() { return fences_; }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;

  display::ImageId next_image_id_ = display::ImageId(1000);
  display::DriverImageId next_driver_image_id_ = display::DriverImageId(2000);

  StubImageLifecycleListener image_lifecycle_listener_;
  FakeFenceListener fence_listener_{&waiting_images_};
  WaitingImageList waiting_images_;

  FenceCollection fences_;
};

TEST_F(WaitingImageListTest, AddTooManyImages) {
  fbl::RefPtr<Image> image = CreateImage();

  // Add maximum number.
  for (size_t i = 0; i < WaitingImageList::kMaxSize; ++i) {
    EXPECT_OK(waiting_images().PushImage(image, nullptr));
  }

  // Try to add one too many.
  {
    zx::result<> result = waiting_images().PushImage(image, nullptr);
    ASSERT_FALSE(result.is_ok());
    EXPECT_STATUS(result.status_value(), ZX_ERR_BAD_STATE);
    EXPECT_EQ(waiting_images().size(), WaitingImageList::kMaxSize);
  }
}

TEST_F(WaitingImageListTest, RetireSpecificImage) {
  fbl::RefPtr<Image> image1 = CreateImage();
  fbl::RefPtr<Image> image2 = CreateImage();

  // We will add each image twice, in all permutations:
  // 1 1 2 2
  // 1 2 1 2
  // 1 2 2 1
  // 2 1 1 2
  // 2 1 2 1
  // 2 2 1 1

  static_assert(WaitingImageList::kMaxSize >= 4);

  // 1 1 2 2
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_EQ(waiting_images().size(), 4u);
  waiting_images().RemoveImage(*image1);
  EXPECT_EQ(waiting_images().size(), 2u);
  waiting_images().RemoveImage(*image2);
  EXPECT_EQ(waiting_images().size(), 0u);

  // 1 2 1 2
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_EQ(waiting_images().size(), 4u);
  waiting_images().RemoveImage(*image1);
  EXPECT_EQ(waiting_images().size(), 2u);
  waiting_images().RemoveImage(*image2);
  EXPECT_EQ(waiting_images().size(), 0u);

  // 1 2 2 1
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_EQ(waiting_images().size(), 4u);
  waiting_images().RemoveImage(*image1);
  EXPECT_EQ(waiting_images().size(), 2u);
  waiting_images().RemoveImage(*image2);
  EXPECT_EQ(waiting_images().size(), 0u);

  // 2 1 1 2
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_EQ(waiting_images().size(), 4u);
  waiting_images().RemoveImage(*image1);
  EXPECT_EQ(waiting_images().size(), 2u);
  waiting_images().RemoveImage(*image2);
  EXPECT_EQ(waiting_images().size(), 0u);

  // 2 1 2 1
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_EQ(waiting_images().size(), 4u);
  waiting_images().RemoveImage(*image1);
  EXPECT_EQ(waiting_images().size(), 2u);
  waiting_images().RemoveImage(*image2);
  EXPECT_EQ(waiting_images().size(), 0u);

  // 2 2 1 1
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  EXPECT_EQ(waiting_images().size(), 4u);
  waiting_images().RemoveImage(*image1);
  EXPECT_EQ(waiting_images().size(), 2u);
  waiting_images().RemoveImage(*image2);
  EXPECT_EQ(waiting_images().size(), 0u);
}

TEST_F(WaitingImageListTest, AddAndRemoveImages) {
  std::array<fbl::RefPtr<Image>, 3> images = {CreateImage(), CreateImage(), CreateImage()};

  static_assert(WaitingImageList::kMaxSize >= 5);

  // Cycle through the images, adding them until the max count is reached.
  for (size_t i = 0; i < WaitingImageList::kMaxSize; ++i) {
    EXPECT_TRUE(waiting_images().PushImage(images[i % 3], nullptr).is_ok());
  }

  // Now that `waiting_images` is full, the next attempts to add should fail.
  // Of course, it doesn't matter which image we try to add;
  EXPECT_FALSE(waiting_images().PushImage(images[0], nullptr).is_ok());
  EXPECT_FALSE(waiting_images().PushImage(images[1], nullptr).is_ok());
  EXPECT_FALSE(waiting_images().PushImage(images[2], nullptr).is_ok());

  // If we remove the oldest 3, we can add three more, but not a 4th.
  // Also, because we're removing the oldest, the newest image doesn't change.
  auto newest_image = waiting_images().GetNewestImageForTesting();
  waiting_images().RemoveOldestImages(2);
  EXPECT_EQ(waiting_images().size(), WaitingImageList::kMaxSize - 2);
  EXPECT_EQ(newest_image, waiting_images().GetNewestImageForTesting());

  EXPECT_TRUE(waiting_images().PushImage(images[0], nullptr).is_ok());
  EXPECT_EQ(waiting_images().size(), WaitingImageList::kMaxSize - 1);
  EXPECT_EQ(images[0], waiting_images().GetNewestImageForTesting());

  EXPECT_TRUE(waiting_images().PushImage(images[1], nullptr).is_ok());
  EXPECT_EQ(waiting_images().size(), WaitingImageList::kMaxSize);
  EXPECT_EQ(images[1], waiting_images().GetNewestImageForTesting());

  EXPECT_FALSE(waiting_images().PushImage(images[2], nullptr).is_ok());
  EXPECT_EQ(images[1], waiting_images().GetNewestImageForTesting());
}

TEST_F(WaitingImageListTest, ReadyImages) {
  // Create images used by the test.
  auto image1 = CreateImage();
  auto image2 = CreateImage();
  auto image3 = CreateImage();

  // Create and import events used by the test.
  constexpr display::EventId kWaitFenceId_1(1);
  constexpr display::EventId kWaitFenceId_2(2);
  constexpr display::EventId kWaitFenceId_3(3);
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(fences().ImportEvent(std::move(event), kWaitFenceId_1));
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(fences().ImportEvent(std::move(event), kWaitFenceId_2));
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(fences().ImportEvent(std::move(event), kWaitFenceId_3));
  auto fence_release = fit::defer([&]() mutable {
    fences().ReleaseEvent(kWaitFenceId_1);
    fences().ReleaseEvent(kWaitFenceId_2);
    fences().ReleaseEvent(kWaitFenceId_3);
  });

  // All images are ready.
  ASSERT_OK(waiting_images().PushImage(image1, nullptr));
  ASSERT_OK(waiting_images().PushImage(image2, nullptr));
  ASSERT_OK(waiting_images().PushImage(image3, nullptr));
  // Latest image will be popped.  Afterward, older images will be discarded.
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image3);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);
  EXPECT_EQ(waiting_images().size(), 0U);

  // One image is ready.
  ASSERT_OK(waiting_images().PushImage(image1, fences().GetFence(kWaitFenceId_1)));
  ASSERT_OK(waiting_images().PushImage(image2, nullptr));
  ASSERT_OK(waiting_images().PushImage(image3, fences().GetFence(kWaitFenceId_3)));

  // The only ready image will be popped, and older ones discarded.  The newest image will remain,
  // because it is both newer and waiting on a fence.
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image2);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);
  {
    auto remaining = waiting_images().GetFullContentsForTesting();
    ASSERT_EQ(remaining.size(), 1U);
    EXPECT_EQ(remaining[0].image(), image3);
  }
  // Continuing from the current state, if we add two more images they will be in the order 3 1 2,
  // all with unsignaled fences.
  ASSERT_OK(waiting_images().PushImage(image1, fences().GetFence(kWaitFenceId_1)));
  ASSERT_OK(waiting_images().PushImage(image2, fences().GetFence(kWaitFenceId_2)));
  {
    auto remaining = waiting_images().GetFullContentsForTesting();
    ASSERT_EQ(remaining.size(), 3U);
    // 3 1 2
    EXPECT_EQ(remaining[0].image(), image3);
    EXPECT_EQ(remaining[1].image(), image1);
    EXPECT_EQ(remaining[2].image(), image2);
    // 3 1 2
    EXPECT_EQ(remaining[0].wait_fence(), fences().GetFence(kWaitFenceId_3));
    EXPECT_EQ(remaining[1].wait_fence(), fences().GetFence(kWaitFenceId_1));
    EXPECT_EQ(remaining[2].wait_fence(), fences().GetFence(kWaitFenceId_2));
  }
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);

  // Signal third fence. Because the third image is the oldest (since the first/second images were
  // retired, then re-added), it will be the only one popped/removed.
  fences().GetFence(kWaitFenceId_3)->Signal();
  driver_runtime_.RunUntilIdle();
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image3);
  {
    auto remaining = waiting_images().GetFullContentsForTesting();
    ASSERT_EQ(remaining.size(), 2U);
    EXPECT_EQ(remaining[0].image(), image1);
    EXPECT_EQ(remaining[1].image(), image2);
  }
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);

  // Signal the second fence. Because the second image is the newest (even newer than the first
  // image), the first image will be retired and the second image will be popped.
  fences().GetFence(kWaitFenceId_2)->Signal();
  driver_runtime_.RunUntilIdle();
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image2);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);
  EXPECT_EQ(waiting_images().size(), 0U);
}

TEST_F(WaitingImageListTest, AddSameImage) {
  // Create image used by the test.
  auto image = CreateImage();

  // Create and import events used by the test.
  constexpr display::EventId kWaitFenceId_1(1);
  constexpr display::EventId kWaitFenceId_2(2);
  constexpr display::EventId kWaitFenceId_3(3);
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(fences().ImportEvent(std::move(event), kWaitFenceId_1));
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(fences().ImportEvent(std::move(event), kWaitFenceId_2));
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_OK(fences().ImportEvent(std::move(event), kWaitFenceId_3));
  auto fence_release = fit::defer([&]() mutable {
    fences().ReleaseEvent(kWaitFenceId_1);
    fences().ReleaseEvent(kWaitFenceId_2);
    fences().ReleaseEvent(kWaitFenceId_3);
  });

  ASSERT_OK(waiting_images().PushImage(image, nullptr));
  ASSERT_OK(waiting_images().PushImage(image, fences().GetFence(kWaitFenceId_1)));
  ASSERT_OK(waiting_images().PushImage(image, fences().GetFence(kWaitFenceId_2)));
  ASSERT_OK(waiting_images().PushImage(image, fences().GetFence(kWaitFenceId_3)));

  // Image can only be popped once, because the other additions are blocked on fences.
  EXPECT_EQ(waiting_images().size(), 4U);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);
  EXPECT_EQ(waiting_images().size(), 3U);

  // Signal the oldest fence.
  fences().GetFence(kWaitFenceId_1)->Signal();
  driver_runtime_.RunUntilIdle();
  EXPECT_EQ(waiting_images().size(), 3U);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);
  EXPECT_EQ(waiting_images().size(), 2U);

  // Signal the newest (i.e. 3rd) fence.  Popping the newest image also removes the entry associated
  // with the 2nd fence because although unsignaled, it is older than the entry associated with the
  // 3rd fence.
  fences().GetFence(kWaitFenceId_3)->Signal();
  driver_runtime_.RunUntilIdle();
  EXPECT_EQ(waiting_images().size(), 2U);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), image);
  EXPECT_EQ(waiting_images().PopNewestReadyImage(), nullptr);
  EXPECT_EQ(waiting_images().size(), 0U);
}

TEST_F(WaitingImageListTest, UpdateLatestClientConfigStamp) {
  // Create images used by the test.
  auto image1 = CreateImage();
  auto image2 = CreateImage();

  display::ConfigStamp stamp(1);

  // It's OK to update the stamp when there are no images; there will be no effect.
  waiting_images().UpdateLatestClientConfigStamp(++stamp);

  // Add an image and update its stamp.
  EXPECT_OK(waiting_images().PushImage(image1, nullptr));
  waiting_images().UpdateLatestClientConfigStamp(++stamp);
  EXPECT_EQ(waiting_images().GetNewestImageForTesting()->latest_client_config_stamp(),
            display::ConfigStamp(3));
  waiting_images().UpdateLatestClientConfigStamp(++stamp);
  EXPECT_EQ(waiting_images().GetNewestImageForTesting()->latest_client_config_stamp(),
            display::ConfigStamp(4));

  // Add another image.  Now it will be the one whose stamp is updated, not the older image.
  EXPECT_OK(waiting_images().PushImage(image2, nullptr));
  waiting_images().UpdateLatestClientConfigStamp(++stamp);
  {
    auto remaining = waiting_images().GetFullContentsForTesting();
    ASSERT_EQ(remaining.size(), 2U);
    EXPECT_EQ(remaining[0].image(), image1);
    EXPECT_EQ(remaining[1].image(), image2);
    EXPECT_EQ(remaining[0].image()->latest_client_config_stamp(), display::ConfigStamp(4));
    EXPECT_EQ(remaining[1].image()->latest_client_config_stamp(), display::ConfigStamp(5));
  }

  // If we retire image2, then image1 will be the latest remaining image.
  waiting_images().RemoveImage(*image2);
  waiting_images().UpdateLatestClientConfigStamp(++stamp);
  {
    auto remaining = waiting_images().GetFullContentsForTesting();
    ASSERT_EQ(remaining.size(), 1U);
    EXPECT_EQ(remaining[0].image(), image1);
    EXPECT_EQ(remaining[0].image()->latest_client_config_stamp(), display::ConfigStamp(6));
  }
}

}  // namespace

}  // namespace display_coordinator
