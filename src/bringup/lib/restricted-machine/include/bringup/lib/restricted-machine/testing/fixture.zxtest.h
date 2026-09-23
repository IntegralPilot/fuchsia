// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_FIXTURE_ZXTEST_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_FIXTURE_ZXTEST_H_

#include <zxtest/zxtest.h>

namespace restricted_machine {

namespace testing {

// Wrap the few zxtest and googletest differences.
template <typename Param>
using TestWithParam = zxtest::TestWithParam<Param>;
#define GTEST_SKIP ZXTEST_SKIP
template <typename ParamType>
using TestParamInfo = zxtest::TestParamInfo<ParamType>;

}  // namespace testing
}  // namespace restricted_machine

#include "fixture.h"

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_FIXTURE_ZXTEST_H_
