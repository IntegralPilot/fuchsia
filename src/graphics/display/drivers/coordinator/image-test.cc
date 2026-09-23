// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/image.h"

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/testing/mock-image-lifecycle-listener.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"

namespace display_coordinator {

class ImageTest : public ::testing::Test {
 public:
  void TearDown() override { lifecycle_listener_.CheckAllCallsReplayed(); }

 protected:
  testing::MockImageLifecycleListener lifecycle_listener_;
};

TEST_F(ImageTest, LifecycleListenerCalled) {
  static constexpr ClientId kClientId(1000);
  static constexpr display::DriverImageId kDriverImageId(2000);
  static constexpr display::ImageId kImageId(3000);
  static constexpr display::ImageMetadata kImageMetadata({
      .width = 100,
      .height = 200,
      .tiling_type = display::ImageTilingType::kLinear,
  });

  lifecycle_listener_.ExpectImageWillBeDestroyed(
      [&](display::DriverImageId driver_image_id) { EXPECT_EQ(driver_image_id, kDriverImageId); });

  fbl::RefPtr<Image> image = fbl::AdoptRef(new Image(&lifecycle_listener_, kImageMetadata, kImageId,
                                                     kDriverImageId, nullptr, kClientId));
}

}  // namespace display_coordinator
