// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_VIEW_TESTS_MOCKS_MOCK_ACCESSIBILITY_VIEW_H_
#define SRC_UI_A11Y_LIB_VIEW_TESTS_MOCKS_MOCK_ACCESSIBILITY_VIEW_H_

#include <optional>

#include "src/ui/a11y/lib/view/accessibility_view.h"

namespace accessibility_test {

class MockAccessibilityView : public a11y::AccessibilityViewInterface {
 public:
  MockAccessibilityView() = default;
  ~MockAccessibilityView() override = default;

  // |AccessibilityViewInterface|
  std::optional<fuchsia::ui::views::ViewRef> view_ref() override {
    return std::move(a11y_view_ref_);
  }

  void set_view_ref(std::optional<fuchsia::ui::views::ViewRef> view_ref) {
    a11y_view_ref_ = std::move(view_ref);
  }

  // |AccessibilityViewInterface|
  void add_view_properties_changed_callback(ViewPropertiesChangedCallback callback) override {
    view_properties_changed_callback_ = std::move(callback);
  }

  // |AccessibilityViewInterface|
  void add_scene_ready_callback(SceneReadyCallback callback) override {
    scene_ready_callback_ = std::move(callback);
  }

  void invoke_scene_ready_callback() { scene_ready_callback_(); }

  // |AccessibilityViewInterface|
  void RequestFocus(fuchsia::ui::views::ViewRef view_ref, RequestFocusCallback callback) override {
    focused_view_ref_ = std::move(view_ref);
    focus_callback_ = std::move(callback);
  }

  std::optional<fuchsia::ui::views::ViewRef> focused_view_ref() {
    return std::move(focused_view_ref_);
  }

  void invoke_focus_callback(fuchsia::ui::views::Focuser_RequestFocus_Result value) {
    focus_callback_(std::move(value));
  }

  // |AccessibilityViewInterface|
  fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr TakeTouchSource() override {
    return std::move(touch_source_);
  }

  // |AccessibilityViewInterface|
  void SetTouchSource(
      fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr touch_source) override {
    touch_source_ = std::move(touch_source);
  }

 private:
  std::optional<fuchsia::ui::views::ViewRef> a11y_view_ref_;
  ViewPropertiesChangedCallback view_properties_changed_callback_;
  SceneReadyCallback scene_ready_callback_;
  RequestFocusCallback focus_callback_;
  std::optional<fuchsia::ui::views::ViewRef> focused_view_ref_;
  fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr touch_source_;
};

}  // namespace accessibility_test

#endif  // SRC_UI_A11Y_LIB_VIEW_TESTS_MOCKS_MOCK_ACCESSIBILITY_VIEW_H_
