// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_BASE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_BASE_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fit/function.h>
#include <lib/zx/bti.h>
#include <lib/zx/time.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/fake-display-stack/fake-display-stack.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display.h"

namespace display_coordinator {

class TestBase : public testing::Test {
 public:
  TestBase();
  ~TestBase() override;

  void SetUp() override;
  void TearDown() override;

  fake_display::FakeDisplay& FakeDisplayEngine();

  fidl::ClientEnd<fuchsia_sysmem2::Allocator> ConnectToSysmemAllocatorV2();
  fidl::WireSyncClient<fuchsia_hardware_display::Provider> DisplayProviderClient();

  async_dispatcher_t* dispatcher() { return loop_.dispatcher(); }

  // Runs the Driver Runtime foreground dispatcher until a condition is met.
  //
  // `predicate` will only be evaluated on the calling thread.
  //
  // The method does not return if `predicate` never returns true. The test will
  // either time out or run indefinitely.
  void WaitUntil(fit::function<bool()> predicate);

 private:
  async::Loop loop_;

  std::unique_ptr<fake_display::FakeDisplayStack> fake_display_stack_;

  fidl::ClientEnd<fuchsia_io::Directory> incoming_root_directory_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_BASE_H_
