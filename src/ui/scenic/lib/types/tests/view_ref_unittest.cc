// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/view_ref.h"

#include <gtest/gtest.h>

namespace types {
namespace {

TEST(ViewRefTest, Equality) {
  // Different FIDL view-refs result in inequality.
  {
    fuchsia_ui_views::ViewRef fidl_view_ref_1;
    fuchsia_ui_views::ViewRef fidl_view_ref_2;
    zx::eventpair control_ref_1, control_ref_2;
    ASSERT_EQ(ZX_OK, zx::eventpair::create(0u, &control_ref_1, &fidl_view_ref_1.reference()));
    ASSERT_EQ(ZX_OK, zx::eventpair::create(0u, &control_ref_2, &fidl_view_ref_2.reference()));

    ViewRef view_ref1(std::move(fidl_view_ref_1));
    ViewRef view_ref2(std::move(fidl_view_ref_1));

    EXPECT_NE(view_ref1, view_ref2);
  }
  // Matching FIDL view-refs result in equality.
  {
    fuchsia_ui_views::ViewRef fidl_view_ref_1;
    fuchsia_ui_views::ViewRef fidl_view_ref_2;
    zx::eventpair control_ref, control_ref_2;
    ASSERT_EQ(ZX_OK, zx::eventpair::create(0u, &control_ref, &fidl_view_ref_1.reference()));
    ASSERT_EQ(ZX_OK, fidl_view_ref_1.reference().duplicate(ZX_RIGHT_SAME_RIGHTS,
                                                           &fidl_view_ref_2.reference()));

    ViewRef view_ref1(std::move(fidl_view_ref_1));
    ViewRef view_ref2(std::move(fidl_view_ref_2));

    EXPECT_EQ(view_ref1, view_ref2);
  }

  // Null FIDL view-refs result in equality.
  {
    fuchsia_ui_views::ViewRef fidl_view_ref_1;
    fuchsia_ui_views::ViewRef fidl_view_ref_2;

    ViewRef view_ref1(std::move(fidl_view_ref_1));
    ViewRef view_ref2(std::move(fidl_view_ref_2));

    EXPECT_EQ(view_ref1, view_ref2);
  }
}

}  // namespace
}  // namespace types
