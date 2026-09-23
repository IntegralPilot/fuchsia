// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/accessibility/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <memory>

#include <gtest/gtest.h>
#include <test/accessibility/cpp/fidl.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/ui/testing/ui_test_manager/ui_test_manager.h"
#include "src/ui/testing/util/test_view.h"

namespace integration_tests {

using component_testing::ChildRef;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Realm;
using component_testing::Route;

constexpr auto kViewProvider = "view-provider";

// These tests leverage the coordinate test view to ensure that Scene Manager
// magnification APIs are working properly.
// ___________________________________
// |                |                |
// |     BLACK      |        RED     |
// |           _____|_____           |
// |___________|  GREEN  |___________|
// |           |_________|           |
// |                |                |
// |      BLUE      |     MAGENTA    |
// |________________|________________|
//
// These are rough integration tests to supplement the |ScenicPixelTest| clip-space transform tests.
class MagnificationPixelTest : public gtest::RealLoopFixture {
 protected:
  // |testing::Test|
  void SetUp() override {
    ui_testing::UITestRealm::Config config;
    config.use_scene_owner = true;
    config.accessibility_owner = ui_testing::UITestRealm::AccessibilityOwnerType::FAKE;
    config.ui_to_client_services = {fuchsia::ui::composition::FlatlandFactory::Name_};
    ui_test_manager_.emplace(std::move(config));

    // Build realm.
    FX_LOGS(INFO) << "Building realm";
    realm_ = ui_test_manager_->AddSubrealm();

    // Add a test view provider.
    test_view_access_ = std::make_shared<ui_testing::TestViewAccess>();
    component_testing::LocalComponentFactory test_view = [d = dispatcher(),
                                                          a = test_view_access_]() {
      return std::make_unique<ui_testing::TestView>(
          d, /* content = */ ui_testing::TestView::ContentType::COORDINATE_GRID, a);
    };
    realm_->AddLocalChild(kViewProvider, std::move(test_view));

    realm_->AddRoute(Route{.capabilities = {Protocol{fuchsia::ui::app::ViewProvider::Name_}},
                           .source = ChildRef{kViewProvider},
                           .targets = {ParentRef()}});
    realm_->AddRoute(
        Route{.capabilities = {Protocol{fuchsia::ui::composition::FlatlandFactory::Name_}},
              .source = ParentRef(),
              .targets = {ChildRef{kViewProvider}}});

    ui_test_manager_->BuildRealm();
    realm_exposed_services_ = ui_test_manager_->CloneExposedServicesDirectory();

    // Attach view, and wait for it to render.
    ui_test_manager_->InitializeScene();
    RunLoopUntil([this]() { return ui_test_manager_->ClientViewIsRendering(); });

    fake_magnifier_ = realm_exposed_services_->Connect<test::accessibility::Magnifier>();
  }

  void TearDown() override {
    bool complete = false;
    ui_test_manager_->TeardownRealm(
        [&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  }

  void SetClipSpaceTransform(float scale, float x, float y) {
    // HACK HACK HACK
    // TODO(https://fxbug.dev/42081619): Remove this when we move to the new gesture
    // disambiguation protocols.
    //
    // Because the FlatlandAcessibilityView::SetMagnificationTransform
    // hardcodes translation values at a display rotation of 270, this test
    // must normalize the values given the display rotation of 0.
    //
    // Since a display rotation of 270 uses (x, y) as (y, -x), pass
    // in (y, -x), which will yield (x, y) to get an effective display
    // rotation of 0.
    fake_magnifier_->SetMagnification(scale, y, -x, [this]() { QuitLoop(); });

    RunLoop();
  }

  ui_testing::Screenshot TakeScreenshot() { return ui_test_manager_->TakeScreenshot(); }

 private:
  std::optional<ui_testing::UITestManager> ui_test_manager_;
  std::unique_ptr<sys::ServiceDirectory> realm_exposed_services_;
  std::shared_ptr<ui_testing::TestViewAccess> test_view_access_;
  std::optional<Realm> realm_;

  test::accessibility::MagnifierPtr fake_magnifier_;
};

TEST_F(MagnificationPixelTest, Identity) {
  SetClipSpaceTransform(/* scale = */ 1, /* translation_x = */ 0, /* translation_y = */ 0);

  RunLoopUntil([this]() {
    auto data = TakeScreenshot();

    return data.GetPixelAt(data.width() / 4, data.height() / 4) == utils::kBlack &&
           data.GetPixelAt(data.width() / 4, 3 * data.height() / 4) == utils::kBlue &&
           data.GetPixelAt(3 * data.width() / 4, data.height() / 4) == utils::kRed &&
           data.GetPixelAt(3 * data.width() / 4, 3 * data.height() / 4) == utils::kMagenta &&
           data.GetPixelAt(data.width() / 2, data.height() / 2) == utils::kGreen;
  });
}

TEST_F(MagnificationPixelTest, Center) {
  SetClipSpaceTransform(/* scale = */ 4, /* translation_x = */ 0, /* translation_y = */ 0);

  RunLoopUntil([this]() {
    auto data = TakeScreenshot();

    return data.GetPixelAt(data.width() / 4, data.height() / 4) == utils::kGreen &&
           data.GetPixelAt(data.width() / 4, 3 * data.height() / 4) == utils::kGreen &&
           data.GetPixelAt(3 * data.width() / 4, data.height() / 4) == utils::kGreen &&
           data.GetPixelAt(3 * data.width() / 4, 3 * data.height() / 4) == utils::kGreen &&
           data.GetPixelAt(data.width() / 2, data.height() / 2) == utils::kGreen;
  });
}

TEST_F(MagnificationPixelTest, RotatedUpperLeft) {
  SetClipSpaceTransform(/* scale = */ 2, /* translation_x = */ 1, /* translation_y = */ 1);

  RunLoopUntil([this]() {
    auto data = TakeScreenshot();

    return data.GetPixelAt(data.width() / 4, data.height() / 4) == utils::kBlack &&
           data.GetPixelAt(data.width() / 4, 3 * data.height() / 4) == utils::kBlack &&
           data.GetPixelAt(3 * data.width() / 4, data.height() / 4) == utils::kBlack &&
           data.GetPixelAt(3 * data.width() / 4, 3 * data.height() / 4) == utils::kGreen;
  });
}

}  // namespace integration_tests
