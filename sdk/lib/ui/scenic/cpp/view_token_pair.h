// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UI_SCENIC_CPP_VIEW_TOKEN_PAIR_H_
#define LIB_UI_SCENIC_CPP_VIEW_TOKEN_PAIR_H_

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <fuchsia/ui/views/cpp/fidl.h>
#include <lib/zx/eventpair.h>

namespace scenic {

struct ViewTokenPair {
  // Convenience function which allows clients to easily create a valid
  // |ViewToken| / |ViewHolderToken| pair for use with |View| / |ViewHolder|
  // resources.
  static ViewTokenPair New();

  fuchsia::ui::views::ViewToken view_token;
  fuchsia::ui::views::ViewHolderToken view_holder_token;
};

using ViewTokenStdPair =
    std::pair<fuchsia::ui::views::ViewToken, fuchsia::ui::views::ViewHolderToken>;

// Convenience function which allows clients to easily create a |ViewToken| /
// |ViewHolderToken| pair for use with |View| resources.
ViewTokenStdPair NewViewTokenPair();

// Convenience functions which allow converting from raw eventpair-based tokens
// easily.
// TEMPORARY; for transition purposes only.
// TODO(https://fxbug.dev/42098686): Remove.
fuchsia::ui::views::ViewToken ToViewToken(zx::eventpair raw_token);
fuchsia::ui::views::ViewHolderToken ToViewHolderToken(zx::eventpair raw_token);

namespace cpp {

struct ViewTokenPair {
  // Convenience function which allows clients to easily create a valid
  // |ViewToken| / |ViewHolderToken| pair for use with |View| / |ViewHolder|
  // resources.
  static ViewTokenPair New();

  fuchsia_ui_views::ViewToken view_token;
  fuchsia_ui_views::ViewHolderToken view_holder_token;
};

using ViewTokenStdPair = std::pair<fuchsia_ui_views::ViewToken, fuchsia_ui_views::ViewHolderToken>;

// Convenience function which allows clients to easily create a |ViewToken| /
// |ViewHolderToken| pair for use with |View| resources.
ViewTokenStdPair NewViewTokenPair();

// Convenience functions which allow converting from raw eventpair-based tokens
// easily.
// TEMPORARY; for transition purposes only.
// TODO(https://fxbug.dev/42098686): Remove.
fuchsia_ui_views::ViewToken ToViewToken(zx::eventpair raw_token);
fuchsia_ui_views::ViewHolderToken ToViewHolderToken(zx::eventpair raw_token);

}  // namespace cpp

}  // namespace scenic

#endif  // LIB_UI_SCENIC_CPP_VIEW_TOKEN_PAIR_H_
