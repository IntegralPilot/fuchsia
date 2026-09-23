// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UI_SCENIC_CPP_VIEW_IDENTITY_H_
#define LIB_UI_SCENIC_CPP_VIEW_IDENTITY_H_

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <fuchsia/ui/views/cpp/fidl.h>

namespace scenic {

// For fuchsia.ui.composition.Flatland.CreateView() call.
fuchsia::ui::views::ViewIdentityOnCreation NewViewIdentityOnCreation();

namespace cpp {

// For fuchsia_ui_composition.Flatland.CreateView() call.
fuchsia_ui_views::ViewIdentityOnCreation NewViewIdentityOnCreation();

}  // namespace cpp

}  // namespace scenic

#endif  // LIB_UI_SCENIC_CPP_VIEW_IDENTITY_H_
