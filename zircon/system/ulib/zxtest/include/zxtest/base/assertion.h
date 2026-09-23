// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZXTEST_BASE_ASSERTION_H_
#define ZXTEST_BASE_ASSERTION_H_

#include <span>
#include <string>

#include <zxtest/base/message.h>
#include <zxtest/base/types.h>

namespace zxtest {

// Helper class for handling the error information, plus some logic for printing the correct error
// messages.
class Assertion {
 public:
  Assertion() = delete;
  Assertion(std::string_view desc, std::string_view expected, std::string_view expected_eval,
            std::string_view actual, std::string_view actual_eval, const SourceLocation& location,
            bool is_fatal, std::span<zxtest::Message*> traces);
  Assertion(std::string_view desc, const SourceLocation& location, bool is_fatal,
            std::span<zxtest::Message*> traces);
  Assertion(const Assertion&) = delete;
  Assertion(Assertion&&) noexcept;
  ~Assertion();

  Assertion& operator=(const Assertion&) = delete;
  Assertion& operator=(Assertion&&) = delete;

  // Returns the position at which the assertion happened.
  const SourceLocation& location() const { return message_.location(); }

  // Returns a general description of the asserted condition.
  const std::string& description() const { return message_.text(); }

  // Returns the expected value of an equality. For example in ASSERT_EQ(actual, expected) returns
  // the text representation of expected, as it was captured on compile time.
  const std::string& expected() const { return expected_; }

  // Returns the expected value of an equality. For example in ASSERT_EQ(actual, expected) returns
  // the text representation of actual, as it was captured on compile time.
  const std::string& actual() const { return actual_; }

  // Returns the expected value of an equality. For example in ASSERT_EQ(actual, expected) returns
  // the text representation of expected as it is evaluated at runtime..
  const std::string& expected_eval() const { return expected_eval_; }

  // Returns the expected value of an equality. For example in ASSERT_EQ(actual, expected) returns
  // the text representation of actual, as it was captured on runtime.
  const std::string& actual_eval() const { return actual_eval_; }

  // Returns true if this assertion is fatal, and test should stop execution. Essentially if the
  // asserting macro is ASSERT_* or EXPECT_*.
  bool is_fatal() const { return is_fatal_; }

  // Returns true if this assertions is value based or manually generated.
  bool has_values() const { return has_values_; }

  std::span<zxtest::Message*> scoped_traces() const { return traces_; }

 private:
  // Message indicating the nature of the assertion (whether it was expected to be equal, not
  // equal, etc), and the source location.
  Message message_;
  std::string expected_;
  std::string expected_eval_;
  std::string actual_;
  std::string actual_eval_;

  bool is_fatal_;
  bool has_values_;

  std::span<zxtest::Message*> traces_;
};

}  // namespace zxtest

#endif  // ZXTEST_BASE_ASSERTION_H_
