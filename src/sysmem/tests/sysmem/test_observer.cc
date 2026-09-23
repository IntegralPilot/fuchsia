// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test_observer.h"

#include <string>

// This test observer is used to get the name of the current test to send to sysmem to identify the
// client.
std::string current_test_name;
TestObserver test_observer;

void TestObserver::OnTestStart(const zxtest::TestCase& test_case,
                               const zxtest::TestInfo& test_info) {
  current_test_name = std::string(test_case.name()) + "." + std::string(test_info.name());
  if (current_test_name.size() > 64) {
    current_test_name = current_test_name.substr(0, 64);
  }
}
