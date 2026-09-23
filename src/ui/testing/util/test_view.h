// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UTIL_TEST_VIEW_H_
#define SRC_UI_TESTING_UTIL_TEST_VIEW_H_

#include <fuchsia/ui/app/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <fuchsia/ui/views/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_token_pair.h>

#include <optional>

#include <src/lib/fxl/memory/weak_ptr.h>

namespace ui_testing {

class TestView;

// Used to open up access to TestView internals, since LocalComponentImpl (and thus TestView) is not
// accessible directly to the test fixture.
class TestViewAccess {
 public:
  virtual ~TestViewAccess() = default;

  // Must be called by TestView as soon as practical.
  virtual void SetTestView(const fxl::WeakPtr<TestView>& view);

  // Check that view access is possible. Since view creation is lazy, access may
  // not be initially possible for a while.
  bool HasView() const { return test_view_.get() != nullptr; }

  // Since view component creation may be lazy, check first whether HasView()
  // returns true before calling this.
  TestView* view() const {
    FX_CHECK(HasView());
    return test_view_.get();
  }

 protected:
  fxl::WeakPtr<TestView> test_view_{};
};

class TestView final : public fuchsia::ui::app::ViewProvider,
                       public component_testing::LocalComponentImpl {
 public:
  enum class ContentType {
    // Draws a green rect in the view.
    DEFAULT = 0,

    // Draws the following coordinate test pattern in the view:
    //
    // ___________________________________
    // |                |                |
    // |     BLACK      |        RED     |
    // |           _____|_____           |
    // |___________|  GREEN  |___________|
    // |           |_________|           |
    // |                |                |
    // |      BLUE      |     MAGENTA    |
    // |________________|________________|
    COORDINATE_GRID = 1,
  };

  explicit TestView(async_dispatcher_t* dispatcher, ContentType content_type,
                    std::weak_ptr<TestViewAccess> access)
      : dispatcher_(dispatcher),
        content_type_(content_type),
        access_(std::move(access)),
        weak_ptr_factory_(this) {
    if (auto a = access_.lock()) {
      a->SetTestView(weak_ptr_factory_.GetWeakPtr());
    }
  }
  ~TestView() override = default;

  // |component_testing::LocalComponentImpl|
  void OnStart() override;

  const std::optional<fuchsia::ui::views::ViewRef>& view_ref() { return view_ref_; }
  std::optional<zx_koid_t> GetViewRefKoid();

  // |fuchsia.ui.app.ViewProvider|
  void CreateView2(fuchsia::ui::app::CreateView2Args args) override;
  void CreateViewWithViewRef(zx::eventpair token,
                             fuchsia::ui::views::ViewRefControl view_ref_control,
                             fuchsia::ui::views::ViewRef view_ref) override {
    ZX_PANIC("Not Implemented");
  }

  // Add a child view!
  // The viewport will have side length of 1/4 our side length and will be centered in our view.
  void NestChildView();

  uint32_t width();
  uint32_t height();

 private:
  // Helper methods to add content to the view.
  void DrawSimpleBackground();
  void DrawCoordinateGrid();
  void DrawContent();

  // Helper method to draw a rectangle.
  // (x, y, z) specifies the top-left corner of the rect.
  // (width, height) specifies the rect's dimensions.
  // (red, green, blue, alpha) specifies the color.
  void DrawRectangle(int32_t x, int32_t y, int32_t z, uint32_t width, uint32_t height, uint8_t red,
                     uint8_t green, uint8_t blue, uint8_t alpha);

  void PresentChanges();
  void ResizeChildViewport();

  async_dispatcher_t* dispatcher_ = nullptr;
  std::optional<ContentType> content_type_;
  std::weak_ptr<TestViewAccess> access_;
  fidl::BindingSet<fuchsia::ui::app::ViewProvider> view_provider_bindings_;
  std::optional<fuchsia::ui::views::ViewRef> view_ref_;

  // Scene graph:
  // root transform (id=1)
  // --> rectangle holder transform (id=2)
  //     --> ... (optional) rectangles (id=100, 101, 102, ...)
  // --> (optional) child viewport transform (id=3) {content: child viewport id=4}
  const uint64_t kRootTransformId = 1;
  const uint64_t kRectangleHolderTransform = 2;
  const uint64_t kChildViewportTransformId = 3;
  const uint64_t kChildViewportContentId = 4;

  // We'll keep incrementing this to get the next resource id (100, 101, 102, ...)
  uint64_t next_resource_id_ = 100;
  bool child_view_is_nested = false;

  // Scenic session resources.
  fuchsia::ui::composition::FlatlandFactoryPtr flatland_factory_;
  fuchsia::ui::composition::FlatlandPtr flatland_;

  // Used to retrieve a11y view layout info. These should not change over the
  // lifetime of the view.
  fuchsia::ui::composition::ParentViewportWatcherPtr parent_watcher_;

  std::optional<fuchsia::ui::composition::LayoutInfo> layout_info_;

  fxl::WeakPtrFactory<TestView> weak_ptr_factory_;  // Keep last.
};

}  // namespace ui_testing

#endif  // SRC_UI_TESTING_UTIL_TEST_VIEW_H_
