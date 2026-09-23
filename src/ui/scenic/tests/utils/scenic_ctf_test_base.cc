// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"

namespace integration_tests {

void ScenicCtfTest::SetFlatlandDisplayContent(fuchsia_ui_views::ViewportCreationToken token) {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->SetFlatlandDisplayContent(
      std::move(token));
}

void ScenicCtfTest::SetFlatlandDisplayDevicePixelRatio(fuchsia_math::VecF dpr) {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->SetFlatlandDisplayDevicePixelRatio(
      std::move(dpr));
}

const std::shared_ptr<sys::ServiceDirectory>& ScenicCtfTest::LocalServiceDirectory() const {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->LocalServiceDirectory();
}

uint64_t ScenicCtfTest::GetDisplayRotation() const {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->GetDisplayRotation();
}

fuchsia_math::SizeU ScenicCtfTest::GetDisplayDimensions() const {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->GetDisplayDimensions();
}

uint32_t ScenicCtfTest::GetDisplayRefreshRateMillihertz() const {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->GetDisplayRefreshRateMillihertz();
}

uint32_t ScenicCtfTest::GetDisplayMaxLayerCount() const {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->GetDisplayMaxLayerCount();
}

bool ScenicCtfTest::UseDisplayComposition() const {
  return ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->UseDisplayComposition();
}

}  // namespace integration_tests
