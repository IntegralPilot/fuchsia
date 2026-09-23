// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <zircon/assert.h>

#include <algorithm>
#include <cstdlib>
#include <memory>
#include <utility>
#include <vector>

#include <zxtest/base/test-case.h>
#include <zxtest/base/types.h>

namespace zxtest {

namespace {

using internal::SetUpTestCaseFn;
using internal::TearDownTestCaseFn;
using internal::TestDriver;
using internal::TestStatus;

}  // namespace

TestCase::TestCase(std::string_view name, SetUpTestCaseFn set_up, TearDownTestCaseFn tear_down)
    : name_(name), set_up_(std::move(set_up)), tear_down_(std::move(tear_down)) {
  ZX_ASSERT_MSG(set_up_, "Invalid SetUpTestCaseFn");
  ZX_ASSERT_MSG(tear_down_, "Invalid TearDownTestCaseFn");
}
TestCase::TestCase(TestCase&& other) = default;
TestCase::~TestCase() = default;

size_t TestCase::TestCount() const { return test_infos_.size(); }

size_t TestCase::MatchingTestCount() const { return selected_indexes_.size(); }

void TestCase::Filter(TestCase::FilterFn filter) {
  std::vector<size_t> filtered_indexes;
  filtered_indexes.reserve(test_infos_.size());
  for (size_t i = 0; i < test_infos_.size(); ++i) {
    const auto& test_info = test_infos_[i];
    if (!filter || filter(name_, test_info.name())) {
      filtered_indexes.push_back(i);
    }
  }
  selected_indexes_.swap(filtered_indexes);
}

void TestCase::Shuffle(uint32_t random_seed) {
  for (size_t i = 1; i < selected_indexes_.size(); ++i) {
    size_t j = rand_r(&random_seed) % (i + 1);
    if (j != i) {
      std::swap(selected_indexes_[i], selected_indexes_[j]);
    }
  }
}

void TestCase::UnShuffle() {
  // Put the, possibly filtered, list back in order.
  std::ranges::sort(selected_indexes_);
}

bool TestCase::RegisterTest(std::string_view name, const SourceLocation& location,
                            internal::TestFactory factory) {
  auto it = std::ranges::find_if(test_infos_,
                                 [&name](const TestInfo& info) { return info.name() == name; });

  // Test already registered.
  if (it != test_infos_.end()) {
    return false;
  }

  selected_indexes_.push_back(selected_indexes_.size());
  test_infos_.emplace_back(name, location, std::move(factory));
  return true;
}

void TestCase::Run(LifecycleObserver* lifecycle_observer, TestDriver* driver) {
  if (selected_indexes_.empty()) {
    return;
  }

  auto tear_down = fit::defer([this, lifecycle_observer] {
    tear_down_();
    lifecycle_observer->OnTestCaseEnd(*this);
  });
  lifecycle_observer->OnTestCaseStart(*this);
  set_up_();

  if (!driver->Continue()) {
    return;
  }

  for (size_t i : selected_indexes_) {
    const auto& test_info = test_infos_[i];
    {
      // This block enforces that the destructor is called before
      // completing the test, for accurate error reporting.
      // This prevents buggy destructors from crashing after a test completed,
      // marking it as a success.
      lifecycle_observer->OnTestStart(*this, test_info);
      std::unique_ptr<Test> test = test_info.Instantiate(driver);
      test->Run();
    }
    switch (driver->Status()) {
      case TestStatus::kPassed:
        lifecycle_observer->OnTestSuccess(*this, test_info);
        break;
      case TestStatus::kSkipped:
        lifecycle_observer->OnTestSkip(*this, test_info);
        break;
      case TestStatus::kFailed:
        lifecycle_observer->OnTestFailure(*this, test_info);
        if (return_on_failure_) {
          return;
        }
        break;
      default:
        break;
    }
  }
}

}  // namespace zxtest
