// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_TESTING_BUILD_DISPLAY_REALM_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_TESTING_BUILD_DISPLAY_REALM_H_

#include <lib/async/dispatcher.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>

namespace flatland::testing {

struct DisplayRealmConfig {
  // If zero, uses component's default config.
  uint32_t active_width_px = 0;

  // If zero, uses component's default config.
  uint32_t active_height_px = 0;

  // If zero, uses component's default config.
  uint32_t refresh_rate_millihertz = 0;

  // If zero, uses component's default config.
  uint32_t max_layer_count = 0;
};

// Returns a realm that serves the `fuchsia.hardware.display.Service` service
// provided by the fake display stack to the test.
//
// `dispatcher` must be non-null and outlive the lifetime of the constructed
// `RealmRoot`.
component_testing::RealmRoot BuildFakeDisplayRealm(async_dispatcher_t* dispatcher,
                                                   const DisplayRealmConfig& config);

}  // namespace flatland::testing

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_TESTING_BUILD_DISPLAY_REALM_H_
