// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/hanging_get_helper.h"

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>

#include <gtest/gtest.h>

#include "src/ui/scenic/lib/utils/dispatcher_holder.h"

using fuchsia_math::SizeU;
using fuchsia_ui_composition::LayoutInfo;
using fuchsia_ui_composition::ParentViewportStatus;

namespace flatland {
namespace test {

TEST(HangingGetHelperTest, HangingGetProducesValidResponse) {
  HangingGetHelper<SizeU> helper;

  std::optional<SizeU> data;
  helper.SetCallback([&](SizeU d) { data = d; });
  EXPECT_TRUE(helper.HasPendingCallback());
  EXPECT_FALSE(data);

  helper.Update(SizeU{{.width = 1, .height = 2}});

  EXPECT_FALSE(helper.HasPendingCallback());
  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 1, .height = 2}}), data.value());
}

TEST(HangingGetHelperTest, NonHangingGetProducesValidResponse) {
  HangingGetHelper<SizeU> helper;

  helper.Update(SizeU{{.width = 1, .height = 2}});

  std::optional<SizeU> data;
  helper.SetCallback([&](SizeU d) { data = d; });

  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 1, .height = 2}}), data.value());
}

TEST(HangingGetHelperTest, DataOverrideResultsInFinalValue) {
  HangingGetHelper<SizeU> helper;

  helper.Update(SizeU{{.width = 1, .height = 2}});
  helper.Update(SizeU{{.width = 3, .height = 4}});
  helper.Update(SizeU{{.width = 5, .height = 6}});

  std::optional<SizeU> data;
  helper.SetCallback([&](SizeU d) { data = d; });

  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 5, .height = 6}}), data.value());
}

TEST(HangingGetHelperTest, DataOverrideInBatches) {
  HangingGetHelper<SizeU> helper;

  std::optional<SizeU> data;
  helper.SetCallback([&](SizeU d) { data = d; });
  EXPECT_FALSE(data);

  helper.Update(SizeU{{.width = 1, .height = 2}});

  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 1, .height = 2}}), data.value());

  helper.Update(SizeU{{.width = 3, .height = 4}});
  helper.Update(SizeU{{.width = 5, .height = 6}});

  EXPECT_EQ((SizeU{{.width = 1, .height = 2}}), data.value());
  data.reset();
  helper.SetCallback([&](SizeU d) { data = d; });

  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 5, .height = 6}}), data.value());
}

TEST(HangingGetHelperTest, DuplicateDataIsIgnored) {
  HangingGetHelper<SizeU> helper;

  std::optional<SizeU> data;
  helper.SetCallback([&](SizeU d) { data = d; });
  EXPECT_FALSE(data);

  helper.Update(SizeU{{.width = 1, .height = 2}});

  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 1, .height = 2}}), data.value());

  data.reset();
  helper.SetCallback([&](SizeU d) { data = d; });
  EXPECT_FALSE(data);

  helper.Update(SizeU{{.width = 1, .height = 2}});
  EXPECT_FALSE(data);

  helper.Update(SizeU{{.width = 3, .height = 4}});

  ASSERT_TRUE(data);
  EXPECT_EQ((SizeU{{.width = 3, .height = 4}}), data.value());
}

TEST(HangingGetHelperTest, EnumDuplicateDataIsIgnored) {
  HangingGetHelper<ParentViewportStatus> helper;

  std::optional<ParentViewportStatus> data;
  helper.SetCallback([&](ParentViewportStatus d) { data = std::move(d); });

  EXPECT_FALSE(data);

  helper.Update(ParentViewportStatus::kConnectedToDisplay);

  ASSERT_TRUE(data);
  EXPECT_EQ(ParentViewportStatus::kConnectedToDisplay, data.value());

  data.reset();
  helper.SetCallback([&](ParentViewportStatus d) { data = std::move(d); });
  EXPECT_FALSE(data);

  helper.Update(ParentViewportStatus::kConnectedToDisplay);
  EXPECT_FALSE(data);

  helper.Update(ParentViewportStatus::kDisconnectedFromDisplay);

  ASSERT_TRUE(data);
  EXPECT_EQ(ParentViewportStatus::kDisconnectedFromDisplay, data.value());
}

TEST(HangingGetHelperTest, TableDuplicateDataIsIgnored) {
  HangingGetHelper<LayoutInfo> helper;

  std::optional<LayoutInfo> data;
  helper.SetCallback([&](LayoutInfo d) { data = std::move(d); });

  EXPECT_FALSE(data);

  LayoutInfo info;
  info.logical_size(SizeU{{.width = 1, .height = 2}});
  info.device_pixel_ratio(fuchsia_math::VecF{{.x = 2.f, .y = 3.f}});

  helper.Update(info);

  ASSERT_TRUE(data);
  EXPECT_EQ(info, data.value());

  data.reset();
  helper.SetCallback([&](LayoutInfo d) { data = std::move(d); });
  EXPECT_FALSE(data);

  helper.Update(info);
  EXPECT_FALSE(data);

  // Updating just one part of the table is enough for it to not be a duplicate.
  LayoutInfo new_info;
  new_info.logical_size(SizeU{{.width = 5, .height = 6}});

  helper.Update(new_info);

  ASSERT_TRUE(data);
  EXPECT_EQ(new_info, data.value());
}

TEST(HangingGetHelperTest, ViewRefDuplicateDataIsIgnored) {
  HangingGetHelper<fuchsia_ui_views::ViewRef> helper;

  auto pair1 = scenic::cpp::ViewRefPair::New();
  auto pair2 = scenic::cpp::ViewRefPair::New();

  std::optional<fuchsia_ui_views::ViewRef> data;
  helper.SetCallback([&](fuchsia_ui_views::ViewRef d) { data = std::move(d); });

  EXPECT_FALSE(data);

  // Send initial ViewRef.
  helper.Update(scenic::cpp::CloneViewRef(pair1.view_ref));

  ASSERT_TRUE(data);
  EXPECT_TRUE(internal::DataEquals(pair1.view_ref, data.value()));

  // Register a new callback.
  data.reset();
  helper.SetCallback([&](fuchsia_ui_views::ViewRef d) { data = std::move(d); });
  EXPECT_FALSE(data);

  // Updating with a duplicated handle pointing to the same View (same KOID) should be ignored.
  helper.Update(scenic::cpp::CloneViewRef(pair1.view_ref));
  EXPECT_FALSE(data);

  // Updating with a new ViewRef (different KOID) should trigger the callback.
  helper.Update(scenic::cpp::CloneViewRef(pair2.view_ref));

  ASSERT_TRUE(data);
  EXPECT_TRUE(internal::DataEquals(pair2.view_ref, data.value()));
  EXPECT_FALSE(internal::DataEquals(pair1.view_ref, data.value()));
}

}  // namespace test
}  // namespace flatland
