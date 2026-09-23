// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <zircon/errors.h>

#include <iostream>

#include <zxtest/zxtest.h>

#include "lib/stdcompat/string_view.h"

constexpr char kAmlogicDecoderDriver[] =
    "fuchsia-pkg://fuchsia.com/amlogic_decoder#meta/amlogic_video_decoder.cm";

// Test that unbinding and rebinding the driver works.
TEST(TestRunner, Rebind) {
  auto manager = component::Connect<fuchsia_driver_development::Manager>();
  fidl::WireSyncClient manager_client(*std::move(manager));
  auto restart_result = manager_client->RestartDriverHosts(
      fidl::StringView::FromExternal(kAmlogicDecoderDriver),
      fuchsia_driver_development::wire::RestartRematchFlags::kRequested |
          fuchsia_driver_development::wire::RestartRematchFlags::kCompositeSpec);
  ASSERT_TRUE(restart_result.ok());
  EXPECT_TRUE(restart_result->is_ok());
}
