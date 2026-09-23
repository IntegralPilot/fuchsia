// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_VIEW_REF_H_
#define SRC_UI_SCENIC_LIB_TYPES_VIEW_REF_H_

#include <fidl/fuchsia.ui.views/cpp/natural_types.h>

#include <ostream>

namespace types {

// Encapsulate a FIDL ViewRef along with its corresponding KOID.
class ViewRef {
 public:
  explicit ViewRef(fuchsia_ui_views::ViewRef ref);
  // Move-only.
  ViewRef(const ViewRef&) noexcept = delete;
  ViewRef(ViewRef&&) noexcept = default;
  ViewRef& operator=(const ViewRef&) noexcept = delete;
  ViewRef& operator=(ViewRef&&) noexcept = default;

  zx_koid_t koid() const { return koid_; }
  const zx::eventpair& eventpair() const { return ref_.reference(); }

  bool operator==(const ViewRef& other) const { return koid_ == other.koid_; }

 private:
  fuchsia_ui_views::ViewRef ref_;
  zx_koid_t koid_;
};

std::ostream& operator<<(std::ostream& str, const types::ViewRef& vr);

}  // namespace types

#endif  // SRC_UI_SCENIC_LIB_TYPES_VIEW_REF_H_
