// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/testing/scene_provider/scene_provider.h"

#include <fuchsia/session/scene/cpp/fidl.h>
#include <fuchsia/ui/app/cpp/fidl.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <lib/ui/scenic/cpp/view_token_pair.h>
#include <zircon/status.h>

#include "sdk/lib/syslog/cpp/macros.h"
#include "src/lib/fsl/handles/object_info.h"

namespace ui_testing {

void FakeViewController::Dismiss() { dismiss_(); }

SceneProvider::SceneProvider(sys::ComponentContext* context) : context_(context) {
  context_->svc()->Connect(scene_manager_.NewRequest());
}

void SceneProvider::AttachClientView(
    fuchsia::ui::test::scene::ControllerAttachClientViewRequest request,
    AttachClientViewCallback callback) {
  FX_LOGS(INFO) << "Attach client view";

  fuchsia::ui::views::ViewRef client_view_ref;

  fuchsia::session::scene::Manager_SetRootView_Result set_root_view_result;
  zx_status_t status = scene_manager_->SetRootView(std::move(*request.mutable_view_provider()),
                                                   &set_root_view_result);
  FX_CHECK(status == ZX_OK) << "Failed to call SetRootView: " << zx_status_get_string(status);
  FX_CHECK(set_root_view_result.is_response())
      << "Failed to set root view: " << set_root_view_result.err();
  client_view_ref = std::move(set_root_view_result.response().view_ref);

  pending_presented_view_count_++;
  NotifyViewPresentationWatcher();

  callback(fsl::GetKoid(client_view_ref.reference.get()));
}

void SceneProvider::PresentClientView(
    fuchsia::ui::test::scene::ControllerPresentClientViewRequest request) {
  FX_CHECK(request.has_viewport_creation_token()) << "Missing viewport creation token";

  fuchsia::session::scene::Manager_PresentRootView_Result present_root_view_result;
  scene_manager_->PresentRootView(std::move(*request.mutable_viewport_creation_token()),
                                  &present_root_view_result);

  FX_LOGS(INFO) << "Requested to present client view";

  pending_presented_view_count_++;
  NotifyViewPresentationWatcher();
}

void SceneProvider::RegisterViewTreeWatcher(
    fidl::InterfaceRequest<fuchsia::ui::observation::geometry::ViewTreeWatcher> view_tree_watcher,
    RegisterViewTreeWatcherCallback callback) {
  // Register the client's view tree watcher.
  fuchsia::ui::observation::test::RegistrySyncPtr observer_registry;
  context_->svc()->Connect<fuchsia::ui::observation::test::Registry>(
      observer_registry.NewRequest());
  observer_registry->RegisterGlobalViewTreeWatcher(std::move(view_tree_watcher));

  callback();
}

void SceneProvider::WatchViewPresentation(WatchViewPresentationCallback callback) {
  if (view_presentation_callback_ != nullptr) {
    FX_LOGS(ERROR)
        << "Received a WatchViewPresentation request while a prior one was still pending. Ignoring the request.";
    return;
  }

  view_presentation_callback_ = std::move(callback);
  NotifyViewPresentationWatcher();
}

void SceneProvider::NotifyViewPresentationWatcher() {
  if (pending_presented_view_count_ == 0 || !view_presentation_callback_) {
    return;
  }

  view_presentation_callback_();
  view_presentation_callback_ = nullptr;
  pending_presented_view_count_--;
}

// TODO(https://fxbug.dev/42064159): Refactor to accommodate Flatland + Geometry
// Observation.
void SceneProvider::PresentView(
    fuchsia::element::ViewSpec view_spec,
    fidl::InterfaceHandle<fuchsia::element::AnnotationController> annotation_controller,
    fidl::InterfaceRequest<fuchsia::element::ViewController> view_controller,
    PresentViewCallback callback) {
  if (annotation_controller) {
    annotation_controller_.Bind(std::move(annotation_controller));
  }

  if (view_controller) {
    fake_view_controller_.emplace(std::move(view_controller), [this] { this->DismissView(); });
  }

  // TODO(https://fxbug.dev/42057396): Register client's scoped view tree watcher, if
  // requested.

  if (view_spec.has_viewport_creation_token()) {
    fuchsia::session::scene::Manager_PresentRootView_Result set_root_view_result;
    scene_manager_->PresentRootView(std::move(*view_spec.mutable_viewport_creation_token()),
                                    &set_root_view_result);
    if (set_root_view_result.is_err()) {
      FX_LOGS(ERROR) << "fuchsia.session.scene.Manager/PresentRootView error:"
                     << set_root_view_result.err();
      fuchsia::element::GraphicalPresenter_PresentView_Result result;
      result.set_err(fuchsia::element::PresentViewError::INVALID_ARGS);
      callback(std::move(result));
      return;
    }
  } else {
    FX_LOGS(FATAL) << "Invalid view spec";
  }

  pending_presented_view_count_++;
  NotifyViewPresentationWatcher();

  fuchsia::element::GraphicalPresenter_PresentView_Result result;
  result.set_response({});
  callback(std::move(result));
}

fidl::InterfaceRequestHandler<fuchsia::ui::test::scene::Controller>
SceneProvider::GetSceneControllerHandler() {
  return scene_controller_bindings_.GetHandler(this);
}

fidl::InterfaceRequestHandler<fuchsia::element::GraphicalPresenter>
SceneProvider::GetGraphicalPresenterHandler() {
  return graphical_presenter_bindings_.GetHandler(this);
}

void SceneProvider::DismissView() {
  FX_LOGS(FATAL)
      << "Dismissing views on flatland is not yet supported (https://fxbug.dev/42065735)";
}

}  // namespace ui_testing
