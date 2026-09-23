// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/validate_eventpair.h"

#include <lib/syslog/cpp/macros.h>

namespace utils {

bool validate_eventpair(const zx::eventpair& a_object, zx_rights_t a_rights,
                        const zx::eventpair& b_object, zx_rights_t b_rights) {
  if (zx_handle_check_valid(a_object.get()) != ZX_OK) {
    return false;  // bad handle
  }

  if (zx_handle_check_valid(b_object.get()) != ZX_OK) {
    return false;  // bad handle
  }

  zx_info_handle_basic_t a_info{};
  if (a_object.get_info(ZX_INFO_HANDLE_BASIC, &a_info, sizeof(a_info), nullptr, nullptr) != ZX_OK) {
    return false;  // no info
  }
  if (a_info.rights != a_rights) {
    return false;  // unexpected rights
  }

  zx_info_handle_basic_t b_info{};
  if (b_object.get_info(ZX_INFO_HANDLE_BASIC, &b_info, sizeof(b_info), nullptr, nullptr) != ZX_OK) {
    return false;  // no info
  }
  if (b_info.rights != b_rights) {
    return false;  // unexpected rights
  }

  if (a_info.koid != b_info.related_koid) {
    return false;  // unrelated eventpair
  }

  return true;
}

static bool validate_viewref(const zx::eventpair& control_ref, const zx::eventpair& view_ref) {
  const zx_rights_t tight_rights = ZX_DEFAULT_EVENTPAIR_RIGHTS & (~ZX_RIGHT_DUPLICATE);
  bool tight = validate_eventpair(control_ref, tight_rights, view_ref, ZX_RIGHTS_BASIC);
  if (tight) {
    return true;
  }

  bool loose =
      validate_eventpair(control_ref, ZX_DEFAULT_EVENTPAIR_RIGHTS, view_ref, ZX_RIGHTS_BASIC);
  if (loose) {
    FX_LOGS(INFO) << "ViewRefControl is LOOSE.";
    return true;
  }

  FX_LOGS(INFO) << "ViewRefControl is invalid.";
  return false;
}

bool validate_viewref(const fuchsia_ui_views::ViewRefControl& control_ref,
                      const fuchsia_ui_views::ViewRef& view_ref) {
  return validate_viewref(control_ref.reference(), view_ref.reference());
}

bool validate_viewref(const fuchsia_ui_views::wire::ViewRefControl& control_ref,
                      const fuchsia_ui_views::wire::ViewRef& view_ref) {
  return validate_viewref(control_ref.reference, view_ref.reference);
}

}  // namespace utils
