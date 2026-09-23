// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/promise_timeout.h"

#include <lib/async/cpp/executor.h>
#include <lib/fpromise/promise.h>

#include <gtest/gtest.h>

#include "src/developer/forensics/testing/unit_test_fixture.h"

namespace forensics {
namespace {

constexpr zx::duration kTimeout = zx::sec(5);

using PromiseTimeoutTest = UnitTestFixture;

TEST_F(PromiseTimeoutTest, ReturnsValue) {
  constexpr int kExpectedValue = 10;
  auto value_promise = fpromise::make_result_promise<int, Error>(fpromise::ok(kExpectedValue));

  fpromise::promise<int, Error> promise =
      MakePromiseTimeout<int>(std::move(value_promise), dispatcher(), kTimeout);

  std::optional<fpromise::result<int, Error>> out_result;
  async::Executor executor(dispatcher());
  executor.schedule_task(std::move(promise).then(
      [&](fpromise::result<int, Error>& result) { out_result = std::move(result); }));

  RunLoopFor(kTimeout);
  ASSERT_TRUE(out_result.has_value());
  EXPECT_EQ(out_result->value(), kExpectedValue);
}

TEST_F(PromiseTimeoutTest, Timeout) {
  auto value_promise =
      fpromise::make_promise([]() -> fpromise::result<int, Error> { return fpromise::pending(); });

  fpromise::promise<int, Error> promise =
      MakePromiseTimeout<int>(std::move(value_promise), dispatcher(), zx::sec(5));

  std::optional<fpromise::result<int, Error>> out_result;
  async::Executor executor(dispatcher());
  executor.schedule_task(std::move(promise).then(
      [&](fpromise::result<int, Error>& result) { out_result = std::move(result); }));

  RunLoopFor(zx::sec(3));
  EXPECT_FALSE(out_result.has_value());

  RunLoopFor(zx::sec(2));
  ASSERT_TRUE(out_result.has_value());
  EXPECT_EQ(out_result->error(), Error::kTimeout);
}

}  // namespace
}  // namespace forensics
