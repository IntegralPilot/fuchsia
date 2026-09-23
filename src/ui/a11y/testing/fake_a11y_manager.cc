// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/testing/fake_a11y_manager.h"

#include <lib/syslog/cpp/macros.h>

#include "fuchsia/ui/pointer/cpp/fidl.h"

namespace a11y_testing {

FakeSemanticTree::FakeSemanticTree(
    fuchsia::accessibility::semantics::SemanticListenerPtr semantic_listener)
    : semantic_listener_(std::move(semantic_listener)), semantic_tree_binding_(this) {}

void FakeSemanticTree::CommitUpdates(CommitUpdatesCallback callback) { callback(); }

void FakeSemanticTree::Bind(
    fidl::InterfaceRequest<fuchsia::accessibility::semantics::SemanticTree> semantic_tree_request) {
  semantic_tree_binding_.Bind(std::move(semantic_tree_request));
}

void FakeSemanticTree::UpdateSemanticNodes(
    std::vector<fuchsia::accessibility::semantics::Node> nodes) {}

void FakeSemanticTree::DeleteSemanticNodes(std::vector<uint32_t> node_ids) {}

void FakeSemanticTree::SetSemanticsEnabled(bool enabled) {
  semantic_listener_->OnSemanticsModeChanged(enabled, []() {});
}

fidl::InterfaceRequestHandler<fuchsia::accessibility::semantics::SemanticsManager>
FakeA11yManager::GetHandler() {
  return semantics_manager_bindings_.GetHandler(this);
}

void FakeA11yManager::RegisterViewForSemantics(
    fuchsia::ui::views::ViewRef view_ref,
    fidl::InterfaceHandle<fuchsia::accessibility::semantics::SemanticListener> handle,
    fidl::InterfaceRequest<fuchsia::accessibility::semantics::SemanticTree> semantic_tree_request) {
  fuchsia::accessibility::semantics::SemanticListenerPtr semantic_listener;
  semantic_listener.Bind(std::move(handle));
  semantic_trees_.emplace_back(std::make_unique<FakeSemanticTree>(std::move(semantic_listener)));
  semantic_trees_.back()->Bind(std::move(semantic_tree_request));
  semantic_trees_.back()->SetSemanticsEnabled(false);
}

FakeMagnifier::FakeMagnifier(std::unique_ptr<a11y::FlatlandAccessibilityView> a11y_view)
    : a11y_view_(std::move(a11y_view)) {
  a11y_view_->add_scene_ready_callback([this]() {
    // Listen for pointer events.
    touch_source_ = a11y_view_->TakeTouchSource();
    touch_source_->Watch({}, fit::bind_member(this, &FakeMagnifier::WatchCallback));
    return true;
  });
}

void FakeMagnifier::RegisterHandler(
    fidl::InterfaceHandle<fuchsia::accessibility::MagnificationHandler> handler) {
  handler_ = handler.Bind();

  SetMagnificationTransform();
}

void FakeMagnifier::SetMagnification(float scale, float translation_x, float translation_y,
                                     SetMagnificationCallback callback) {
  scale_ = scale;
  translation_x_ = translation_x;
  translation_y_ = translation_y;
  callback_ = std::move(callback);

  SetMagnificationTransform();
}

void FakeMagnifier::SetMagnificationTransform() {
  a11y_view_->SetMagnificationTransform(scale_, translation_x_, translation_y_,
                                        std::move(callback_));
}

fidl::InterfaceRequestHandler<fuchsia::accessibility::Magnifier>
FakeMagnifier::GetMagnifierHandler() {
  return magnifier_bindings_.GetHandler(this);
}

fidl::InterfaceRequestHandler<test::accessibility::Magnifier>
FakeMagnifier::GetTestMagnifierHandler() {
  return test_magnifier_bindings_.GetHandler(this);
}

void FakeMagnifier::WatchCallback(
    std::vector<fuchsia::ui::pointer::augment::TouchEventWithLocalHit> events) {
  // Stores the response for touch events in |events|.
  std::vector<fuchsia::ui::pointer::TouchResponse> responses;
  for (const auto& event : events) {
    if (!event.touch_event.has_pointer_sample()) {
      responses.push_back({});
      continue;
    }

    // Reject all touch events; a11y view is only needed for magnification.
    fuchsia::ui::pointer::TouchResponse response;
    response.set_response_type(fuchsia::ui::pointer::TouchResponseType::NO);
    responses.push_back(std::move(response));
  }

  touch_source_->Watch(std::move(responses), fit::bind_member(this, &FakeMagnifier::WatchCallback));
}

}  // namespace a11y_testing
