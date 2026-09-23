// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/bin/virtual_keyboard_manager/virtual_keyboard_coordinator.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <variant>

#include "fuchsia/input/virtualkeyboard/cpp/fidl.h"
#include "src/ui/bin/virtual_keyboard_manager/virtual_keyboard_controller.h"

namespace virtual_keyboard_manager {

FidlBoundVirtualKeyboardCoordinator::FidlBoundVirtualKeyboardCoordinator(
    sys::ComponentContext* component_context)
    : weak_ptr_factory_(this) {
  FX_DCHECK(component_context);
  component_context->outgoing()
      ->AddPublicService<fuchsia::input::virtualkeyboard::ControllerCreator>(
          [this](
              fidl::InterfaceRequest<fuchsia::input::virtualkeyboard::ControllerCreator> request) {
            creator_bindings_.AddBinding(this, std::move(request), nullptr, [](zx_status_t status) {
              FX_LOGS(INFO) << "controller_creator closed with status=" << status << "( "
                            << zx_status_get_string(status) << ")";
            });
          });
  component_context->outgoing()->AddPublicService<fuchsia::input::virtualkeyboard::Manager>(
      [this](fidl::InterfaceRequest<fuchsia::input::virtualkeyboard::Manager> request) {
        if (manager_binding_) {
          FX_LOGS(WARNING) << "Ignoring Manager interface request; already bound";
        } else {
          BindManager(std::move(request));
        }
      });
}

FidlBoundVirtualKeyboardCoordinator::~FidlBoundVirtualKeyboardCoordinator() {}

// fuchsia.input.virtualkeyboard.ControllerCreator.Create()
void FidlBoundVirtualKeyboardCoordinator::Create(
    fuchsia::ui::views::ViewRef view_ref, fuchsia::input::virtualkeyboard::TextType text_type,
    fidl::InterfaceRequest<fuchsia::input::virtualkeyboard::Controller> controller_request) {
  FX_LOGS(INFO) << "ControllerCreator.Create";
  zx_info_handle_basic_t view_ref_info{};
  zx_koid_t view_koid = ZX_KOID_INVALID;
  if (auto status = view_ref.reference.get_info(ZX_INFO_HANDLE_BASIC, &view_ref_info,
                                                sizeof(view_ref_info), nullptr, nullptr);
      status != ZX_OK) {
    FX_LOGS(ERROR) << __FUNCTION__ << ": failed to get koid for view ref ("
                   << zx_status_get_string(status) << ")";
    return;
  }
  view_koid = view_ref_info.koid;

  for (const auto& binding : controller_bindings_.bindings()) {
    FX_DCHECK(binding->impl());
    if (binding->impl()->view_koid() == view_koid) {
      FX_LOGS(ERROR) << __FUNCTION__ << ": controller already bound for view_koid=" << view_koid;
      controller_request.Close(ZX_ERR_ALREADY_EXISTS);
      return;
    }
  }

  auto controller =
      std::make_unique<FidlBoundVirtualKeyboardController>(GetWeakPtr(), view_koid, text_type);
  controller_bindings_.AddBinding(
      std::move(controller), std::move(controller_request), nullptr,
      [weak_self = GetWeakPtr(), view_koid](zx_status_t status) {
        FX_LOGS(INFO) << "controller for view_koid=" << view_koid
                      << " closed with status=" << status << " (" << zx_status_get_string(status)
                      << ")";
        if (weak_self) {
          weak_self->view_koid_to_pending_manager_config_.erase(view_koid);
        }
      });
}

void FidlBoundVirtualKeyboardCoordinator::NotifyVisibilityChange(
    bool is_visible, fuchsia::input::virtualkeyboard::VisibilityChangeReason reason) {
  FX_LOGS(INFO) << __FUNCTION__;
  if (reason.IsUnknown()) {
    FX_LOGS(WARNING) << __FUNCTION__ << ": ignorning visibility change with reason = " << reason;
    return;
  }

  if (reason == fuchsia::input::virtualkeyboard::VisibilityChangeReason::PROGRAMMATIC) {
    // `Controller` remembers its own changes, so no need to echo them back.
    return;
  }

  FX_DCHECK(reason == fuchsia::input::virtualkeyboard::VisibilityChangeReason::USER_INTERACTION);
  if (!focused_view_koid_.has_value()) {
    return;
  }
  for (const auto& controller : controller_bindings_.bindings()) {
    FX_DCHECK(controller->impl());
    if (controller->impl()->view_koid() == focused_view_koid_.value()) {
      controller->impl()->OnUserAction(is_visible
                                           ? VirtualKeyboardController::UserAction::SHOW_KEYBOARD
                                           : VirtualKeyboardController::UserAction::HIDE_KEYBOARD);
      break;
    }
  }
}

void FidlBoundVirtualKeyboardCoordinator::NotifyManagerError(zx_status_t error) {
  if (manager_binding_) {
    manager_binding_->Close(error);
    manager_binding_.reset();
  } else {
    FX_LOGS(ERROR) << __FUNCTION__ << ": manager_binding_ is empty";
  }
}

void FidlBoundVirtualKeyboardCoordinator::RequestTypeAndVisibility(
    zx_koid_t requestor_view_koid, fuchsia::input::virtualkeyboard::TextType text_type,
    bool is_visible) {
  view_koid_to_pending_manager_config_[requestor_view_koid] =
      KeyboardConfig{.text_type = text_type, .is_visible = is_visible};

  const auto result = ApplyFocusedRequest();
  if (const zx_koid_t* applied_view_koid = std::get_if<zx_koid_t>(&result); applied_view_koid) {
    if (*applied_view_koid != requestor_view_koid) {
      // If `requestor_view_koid` is focused, then `ApplyFocusedRequest()` should have
      // applied the request from `requestor_view_koid`. If some other view is focused,
      // that view's request should have been processed by a prior call to
      // NotifyFocusChange() or BindManager().
      //
      // More to the point: the progress of one controller's request should not depend
      // on requests being made to other controllers.
      FX_LOGS(ERROR) << __FUNCTION__ << " expected to apply request for koid "
                     << requestor_view_koid << " but applied request for koid "
                     << *applied_view_koid;
    }
    FX_LOGS(INFO) << __FUNCTION__ << ": applied";
  } else if (const std::string* reason = std::get_if<std::string>(&result); reason != nullptr) {
    FX_LOGS(INFO) << __FUNCTION__
                  << ": buffered request (requestor_view_koid=" << requestor_view_koid
                  << ", reason=" << *reason << ")";
  } else {
    FX_LOGS(ERROR) << __FUNCTION__ << ": unexpected result variant with index " << result.index();
  }
}

void FidlBoundVirtualKeyboardCoordinator::NotifyFocusChange(
    fuchsia::ui::views::ViewRef focused_view) {
  std::optional<zx_koid_t> new_focused_view_koid;
  zx_info_handle_basic_t view_ref_info{};
  if (auto status = focused_view.reference.get_info(ZX_INFO_HANDLE_BASIC, &view_ref_info,
                                                    sizeof(view_ref_info), nullptr, nullptr);
      status != ZX_OK) {
    FX_LOGS(ERROR) << __FUNCTION__ << ": failed to get koid for view ref ("
                   << zx_status_get_string(status) << ")";
    new_focused_view_koid = ZX_KOID_INVALID;
  } else {
    new_focused_view_koid = view_ref_info.koid;
  }

  // Skip keyboard hiding on spurious focus change.
  if (new_focused_view_koid == focused_view_koid_) {
    return;
  }
  focused_view_koid_ = new_focused_view_koid;

  // Inform manager.
  if (manager_binding_) {
    manager_binding_->impl()->SetVisibility(false);
  } else {
    // When a Manager is (eventually) bound, it will start its keyboard in the hidden state.
    // So we don't need to cache any information here to ensure that the keyboard is hidden.
  }

  // Inform controllers for views that are not the newly focused view.
  for (const auto& controller : controller_bindings_.bindings()) {
    FX_DCHECK(controller->impl());
    if (!focused_view_koid_.has_value() ||
        controller->impl()->view_koid() != focused_view_koid_.value()) {
      controller->impl()->OnUserAction(VirtualKeyboardController::UserAction::HIDE_KEYBOARD);
    }
  }

  if (std::holds_alternative<zx_koid_t>(ApplyFocusedRequest())) {
    FX_LOGS(INFO) << __FUNCTION__ << ": applied pending config";
  }
}

void FidlBoundVirtualKeyboardCoordinator::BindManager(
    fidl::InterfaceRequest<fuchsia::input::virtualkeyboard::Manager> request) {
  FX_LOGS(INFO) << __FUNCTION__;
  // Initialize the VirtualKeyboardManager, using the zero-value of the TextType enum
  // for the initial TextType.
  auto manager = std::make_unique<VirtualKeyboardManager>(
      GetWeakPtr(), fuchsia::input::virtualkeyboard::TextType::ALPHANUMERIC);
  manager_binding_.emplace(std::move(manager), std::move(request));
  manager_binding_->set_error_handler(
      [this](zx_status_t status) { HandleManagerBindingError(status); });
  if (std::holds_alternative<zx_koid_t>(ApplyFocusedRequest())) {
    FX_LOGS(INFO) << __FUNCTION__ << ": applied pending config";
  }
}

void FidlBoundVirtualKeyboardCoordinator::HandleManagerBindingError(zx_status_t status) {
  FX_LOGS(WARNING) << __FUNCTION__ << ": status=" << status << " (" << zx_status_get_string(status)
                   << ")";
  manager_binding_.reset();
  // The VirtualKeyboardManager's demise implies that the keyboard is no
  // longer shown. Inform any listening `VirtualKeyboardController`s about
  // this state change.
  NotifyVisibilityChange(false,
                         fuchsia::input::virtualkeyboard::VisibilityChangeReason::USER_INTERACTION);
}

std::variant<zx_koid_t, std::string> FidlBoundVirtualKeyboardCoordinator::ApplyFocusedRequest() {
  if (!manager_binding_) {
    return "no manager binding";
  }
  if (!focused_view_koid_.has_value()) {
    return "no focused view koid";
  }

  zx_koid_t view_koid = focused_view_koid_.value();
  auto pending_config = view_koid_to_pending_manager_config_.extract(view_koid);
  if (pending_config.empty()) {
    return "no pending config for focused_view_koid_ " + std::to_string(view_koid);
  }
  manager_binding_->impl()->SetTypeAndVisibility(pending_config.mapped().text_type,
                                                 pending_config.mapped().is_visible);
  return view_koid;
}

}  // namespace virtual_keyboard_manager
