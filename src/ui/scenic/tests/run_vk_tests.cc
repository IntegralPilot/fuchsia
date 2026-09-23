// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/scenic_ctf_test_environment.h"

int main(int argc, char** argv) {
  integration_tests::ScenicCtfTestEnvironment::RegisterGlobalTestEnvironment(
      fuchsia_ui_test_context::RendererType::kVulkan);
  return RUN_ALL_TESTS(argc, argv);
}
