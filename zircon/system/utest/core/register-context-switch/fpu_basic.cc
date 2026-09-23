// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <format>

#include <zxtest/zxtest.h>

namespace {

[[gnu::noinline]] double fpu_test_loop(int ex_loops, int factor) {
  double ev[4] = {1.0, -1.0, -1.0, -1.0};
  double t = 0.49999975;

  const int inner = 120 * factor;

  for (int ix = 0; ix < ex_loops; ix++) {
    for (int iz = 0; iz < inner; iz++) {
      ev[0] = (ev[0] + ev[1] + ev[2] - ev[3]) * t;
      ev[1] = (ev[0] + ev[1] - ev[2] + ev[3]) * t;
      ev[2] = (ev[0] - ev[1] + ev[2] + ev[3]) * t;
      ev[3] = (-ev[0] + ev[1] + ev[2] + ev[3]) * t;
    }
    t = 1.0 - t;
  }
  return ev[3];
}

// This is a floating point computation that takes longer than one
// quantum. It is meant to test the code that handles saving and
// restoring the floating point registers, in particular for ARM.
// For reference, with the parameters below it takes about 500ms
// to complete in the arm-qemu-kvm bots.
TEST(RegisterContextSwitchTests, FpuLongComputeLoop) {
  double result = fpu_test_loop(5, 100);
  std::string result_str = std::format("{:3.18f}", result);
  ASSERT_STREQ(result_str, "-1.123982548697285422");
}

}  // namespace
