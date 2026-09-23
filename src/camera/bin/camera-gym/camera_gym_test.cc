// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/camera/bin/camera-gym/stream_cycler.h"

namespace camera {

class CameraGymTest : public testing::Test {
 protected:
  CameraGymTest() = default;
  void SetUp() override {}
  void TearDown() override {}
};

TEST_F(CameraGymTest, PendingCollectionId) {
  StreamCycler cycler(nullptr, false);
  cycler.current_config_index_ = 0;
  cycler.configurations_.emplace_back();
  fuchsia::camera3::StreamProperties2 dummy_properties;
  dummy_properties.set_supports_crop_region(false);
  cycler.configurations_[0].mutable_streams()->push_back(fidl::Clone(dummy_properties));
  cycler.show_buffer_handler_ = [](uint32_t, uint32_t, zx::eventpair*,
                                   std::optional<fuchsia::math::RectF>) { ADD_FAILURE(); };
  cycler.remove_collection_handler_ = [](uint32_t) { ADD_FAILURE(); };
  cycler.DisconnectStream(0);

  constexpr uint32_t kCollectionId = 42;
  cycler.stream_infos_[0].add_collection_handler_returned_value = kCollectionId;

  bool show_ran = false;
  cycler.show_buffer_handler_ = [&](uint32_t collection_id, uint32_t, zx::eventpair*,
                                    std::optional<fuchsia::math::RectF>) {
    EXPECT_EQ(collection_id, kCollectionId);
    show_ran = true;
  };

  fuchsia::camera3::FrameInfo2 frame_info;
  frame_info.set_buffer_index(0);
  zx::eventpair fence;
  zx::eventpair::create(0u, &fence, frame_info.mutable_release_fence());
  cycler.OnNextFrame(0, std::move(frame_info));
  EXPECT_TRUE(show_ran);

  bool remove_ran = false;
  cycler.remove_collection_handler_ = [&](uint32_t collection_id) {
    EXPECT_EQ(collection_id, kCollectionId);
    remove_ran = true;
  };
  fence.reset_and_get_address();
  cycler.DisconnectStream(0);
  EXPECT_TRUE(remove_ran);
}

}  // namespace camera
