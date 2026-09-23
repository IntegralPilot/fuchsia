// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_VIEW_TESTS_MOCKS_SCENIC_MOCKS_H_
#define SRC_UI_A11Y_LIB_VIEW_TESTS_MOCKS_SCENIC_MOCKS_H_

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

#include <set>
#include <unordered_map>
#include <vector>

#include "lib/fidl/cpp/binding.h"

namespace accessibility_test {

class MockLocalHit : public fuchsia::ui::pointer::augment::LocalHit,
                     public fuchsia::ui::pointer::augment::TouchSourceWithLocalHit {
 public:
  MockLocalHit();
  ~MockLocalHit() = default;

  // |fuchsia::ui::pointer::augment::TouchSourceWithLocalHit|
  void Watch(std::vector<fuchsia::ui::pointer::TouchResponse> responses,
             WatchCallback callback) override;

  // |fuchsia::ui::pointer::augment::TouchSourceWithLocalHit|
  void UpdateResponse(fuchsia::ui::pointer::TouchInteractionId interaction,
                      fuchsia::ui::pointer::TouchResponse response,
                      UpdateResponseCallback callback) override;

  uint32_t NumWatchCalls() const;

  void SimulateEvents(std::vector<fuchsia::ui::pointer::augment::TouchEventWithLocalHit> events);

  std::vector<fuchsia::ui::pointer::TouchResponse> TakeResponses();

  std::vector<
      std::pair<fuchsia::ui::pointer::TouchInteractionId, fuchsia::ui::pointer::TouchResponse>>
  TakeUpdatedResponses();

  fidl::InterfaceRequestHandler<fuchsia::ui::pointer::augment::LocalHit> GetHandler(
      async_dispatcher_t* dispatcher = nullptr) {
    return [this,
            dispatcher](fidl::InterfaceRequest<fuchsia::ui::pointer::augment::LocalHit> request) {
      bindings_.AddBinding(this, std::move(request), dispatcher);
    };
  }

  // |fuchsia::ui::pointer::augment::LocalHit|
  void Upgrade(fidl::InterfaceHandle<fuchsia::ui::pointer::TouchSource> original,
               fuchsia::ui::pointer::augment::LocalHit::UpgradeCallback callback) override {
    callback(touch_source_binding_.NewBinding(), nullptr);
  }

  // Returns a bound touch source to this object.
  fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr NewTouchSource() {
    fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr touch_source;
    touch_source.Bind(touch_source_binding_.NewBinding());
    return touch_source;
  }

  void EnqueueTapToEvents();

  void SimulateEnqueuedEvents();

  // Configures the view ref koid that will be used to create fake touch events. This corresponds to
  // the view that would be hit by that event.
  void SetViewRefKoidForTouchEvents(uint64_t view_ref_koid);

 private:
  fidl::BindingSet<fuchsia::ui::pointer::augment::LocalHit> bindings_;
  fidl::Binding<fuchsia::ui::pointer::augment::TouchSourceWithLocalHit> touch_source_binding_;
  uint32_t num_watch_calls_ = 0;
  std::vector<fuchsia::ui::pointer::TouchResponse> responses_;
  std::vector<
      std::pair<fuchsia::ui::pointer::TouchInteractionId, fuchsia::ui::pointer::TouchResponse>>
      updated_responses_;
  WatchCallback callback_;
  std::vector<fuchsia::ui::pointer::augment::TouchEventWithLocalHit> enqueued_events_;
  bool view_parameters_sent_ = false;
  uint64_t view_ref_koid_for_hit_ = 0;
};

}  // namespace accessibility_test

#endif  // SRC_UI_A11Y_LIB_VIEW_TESTS_MOCKS_SCENIC_MOCKS_H_
