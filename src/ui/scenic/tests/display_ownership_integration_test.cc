// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"

namespace integration_tests {

using fuci_DisplayOwnership = fuchsia_ui_composition_internal::DisplayOwnership;

class DisplayOwnershipIntegrationTest : public ScenicCtfTest {
 protected:
  DisplayOwnershipIntegrationTest() = default;

  void SetUp() override {
    ScenicCtfTest::SetUp();
    ownership_ = ConnectSyncIntoRealm<fuci_DisplayOwnership>();
  }

  fidl::SyncClient<fuci_DisplayOwnership> ownership_;
};

TEST_F(DisplayOwnershipIntegrationTest, GetEvent) {
  auto result = ownership_->GetEvent();
  ASSERT_TRUE(result.is_ok());

  EXPECT_TRUE(utils::IsEventSignalled(result->ownership_event(),
                                      fuchsia_ui_composition_internal::kSignalDisplayOwned));
}

}  // namespace integration_tests
