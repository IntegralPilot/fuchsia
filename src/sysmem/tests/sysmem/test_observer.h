// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_TESTS_SYSMEM_TEST_OBSERVER_H_
#define SRC_SYSMEM_TESTS_SYSMEM_TEST_OBSERVER_H_

#include <string>

#include <zxtest/zxtest.h>

class TestObserver : public zxtest::LifecycleObserver {
 public:
  void OnTestStart(const zxtest::TestCase& test_case, const zxtest::TestInfo& test_info) final;
};

extern std::string current_test_name;
extern TestObserver test_observer;

#endif  // SRC_SYSMEM_TESTS_SYSMEM_TEST_OBSERVER_H_
