// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_CONFIG_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_CONFIG_H_

namespace flatland {

// Configures the behavior of a Flatland session.  The default values are those of a normal
// Flatland session (i.e. not created via `TrustedFlatlandFactory`).
struct FlatlandConfig {
  bool schedule_asap = false;
  bool pass_acquire_fences = false;
  bool skips_present_credits = false;
  bool skips_on_frame_presented = false;
  bool use_flatland2 = false;
  bool use_trusted_flatland_api = false;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_CONFIG_H_
