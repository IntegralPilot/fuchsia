// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_SYSCALL_MATCHERS_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_SYSCALL_MATCHERS_H_

// A modified version of gvisor's syscall matchers from test/util/test_util.h.

#include <lib/fit/result.h>
#include <string.h>

#include <type_traits>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace internal {

template <typename E>
class SyscallSuccessMatcher {
 public:
  explicit SyscallSuccessMatcher(E expected) : expected_(::std::move(expected)) {}

  template <typename T>
  operator ::testing::Matcher<T>() const {
    // E is one of three things:
    // - T, or a type losslessly and implicitly convertible to T.
    // - A monomorphic Matcher<T>.
    // - A polymorphic matcher.
    // SafeMatcherCast handles any of the above correctly.
    //
    // Similarly, gMock will invoke this conversion operator to obtain a
    // monomorphic matcher (this is how polymorphic matchers are implemented).
    return ::testing::MakeMatcher(new Impl<T>(::testing::SafeMatcherCast<T>(expected_)));
  }

 private:
  template <typename T>
  class Impl : public ::testing::MatcherInterface<T> {
   public:
    explicit Impl(::testing::Matcher<T> matcher) : matcher_(::std::move(matcher)) {}

    bool MatchAndExplain(T const& rv,
                         ::testing::MatchResultListener* const listener) const override {
      if (rv == static_cast<decltype(rv)>(-1) && errno != 0) {
        *listener << "with errno " << strerror(errno);
        return false;
      }
      bool match = matcher_.MatchAndExplain(rv, listener);
      return match;
    }

    void DescribeTo(::std::ostream* const os) const override { matcher_.DescribeTo(os); }

    void DescribeNegationTo(::std::ostream* const os) const override {
      matcher_.DescribeNegationTo(os);
    }

   private:
    ::testing::Matcher<T> matcher_;
  };

 private:
  E expected_;
};

// A polymorphic matcher equivalent to ::testing::internal::AnyMatcher, except
// not in namespace ::testing::internal, and describing SyscallSucceeds()'s
// match constraints (which are enforced by SyscallSuccessMatcher::Impl).
class AnySuccessValueMatcher {
 public:
  template <typename T>
  operator ::testing::Matcher<T>() const {
    return ::testing::MakeMatcher(new Impl<T>());
  }

 private:
  template <typename T>
  class Impl : public ::testing::MatcherInterface<T> {
   public:
    bool MatchAndExplain(T const& rv,
                         ::testing::MatchResultListener* const listener) const override {
      return true;
    }

    void DescribeTo(::std::ostream* const os) const override { *os << "not -1 (success)"; }

    void DescribeNegationTo(::std::ostream* const os) const override { *os << "-1 (failure)"; }
  };
};

class SyscallFailureMatcher {
 public:
  explicit SyscallFailureMatcher(::testing::Matcher<int> errno_matcher)
      : errno_matcher_(std::move(errno_matcher)) {}

  template <typename T>
  bool MatchAndExplain(T const& rv, ::testing::MatchResultListener* const listener) const {
    if (rv != static_cast<decltype(rv)>(-1)) {
      return false;
    }
    int actual_errno = errno;
    *listener << "with errno " << strerror(actual_errno);
    bool match = errno_matcher_.MatchAndExplain(actual_errno, listener);
    return match;
  }

  void DescribeTo(::std::ostream* const os) const {
    *os << "-1 (failure), with errno ";
    errno_matcher_.DescribeTo(os);
  }

  void DescribeNegationTo(::std::ostream* const os) const {
    *os << "not -1 (success), with errno ";
    errno_matcher_.DescribeNegationTo(os);
  }

 private:
  ::testing::Matcher<int> errno_matcher_;
};

class SpecificErrnoMatcher : public ::testing::MatcherInterface<int> {
 public:
  explicit SpecificErrnoMatcher(int const expected) : expected_(expected) {}

  bool MatchAndExplain(int const actual_errno,
                       ::testing::MatchResultListener* const listener) const override {
    return actual_errno == expected_;
  }

  void DescribeTo(::std::ostream* const os) const override { *os << strerror(expected_); }

  void DescribeNegationTo(::std::ostream* const os) const override {
    *os << "not " << strerror(expected_);
  }

 private:
  int const expected_;
};

inline ::testing::Matcher<int> SpecificErrno(int const expected) {
  return ::testing::MakeMatcher(new SpecificErrnoMatcher(expected));
}

class SyscallResultFailureMatcher {
 public:
  explicit SyscallResultFailureMatcher(::testing::Matcher<int> errno_matcher)
      : errno_matcher_(std::move(errno_matcher)) {}

  template <typename ResultType>
  bool MatchAndExplain(const ResultType& result, ::testing::MatchResultListener* listener) const {
    if (result.is_ok()) {
      *listener << "which succeeded (is fit::ok)";
      return false;
    }
    int actual_errno = static_cast<int>(result.error_value());
    *listener << "with errno " << strerror(actual_errno) << " (" << actual_errno << ")";
    return errno_matcher_.MatchAndExplain(actual_errno, listener);
  }

  void DescribeTo(::std::ostream* os) const {
    *os << "is fit::error with errno ";
    errno_matcher_.DescribeTo(os);
  }

  void DescribeNegationTo(::std::ostream* os) const {
    *os << "is not fit::error with errno ";
    errno_matcher_.DescribeNegationTo(os);
  }

 private:
  ::testing::Matcher<int> errno_matcher_;
};

class SyscallResultSuccessMatcher {
 public:
  template <typename ResultType>
  bool MatchAndExplain(const ResultType& result, ::testing::MatchResultListener* listener) const {
    if (result.is_error()) {
      int actual_errno = static_cast<int>(result.error_value());
      *listener << "failed with errno " << strerror(actual_errno) << " (" << actual_errno << ")";
      return false;
    }
    return true;
  }

  void DescribeTo(::std::ostream* os) const { *os << "is fit::ok"; }
  void DescribeNegationTo(::std::ostream* os) const { *os << "is fit::error"; }
};

template <typename E>
class SyscallResultSuccessWithValueMatcher {
 public:
  explicit SyscallResultSuccessWithValueMatcher(E expected) : expected_(::std::move(expected)) {}

  template <typename ResultType>
  bool MatchAndExplain(const ResultType& result,
                       ::testing::MatchResultListener* const listener) const {
    if (result.is_error()) {
      int actual_errno = static_cast<int>(result.error_value());
      *listener << "failed with errno " << strerror(actual_errno) << " (" << actual_errno << ")";
      return false;
    }
    if constexpr (requires { typename std::remove_cvref_t<ResultType>::value_type; }) {
      using ValueType = typename std::remove_cvref_t<ResultType>::value_type;
      auto matcher = ::testing::SafeMatcherCast<ValueType>(expected_);
      return matcher.MatchAndExplain(result.value(), listener);
    } else {
      *listener << "which has no value to match";
      return false;
    }
  }

  void DescribeTo(::std::ostream* const os) const {
    *os << "is fit::ok with value ";
    if constexpr (requires { expected_.DescribeTo(os); }) {
      expected_.DescribeTo(os);
    } else if constexpr (requires { expected_.impl().DescribeTo(os); }) {
      expected_.impl().DescribeTo(os);
    } else {
      ::testing::internal::UniversalPrinter<E>::Print(expected_, os);
    }
  }

  void DescribeNegationTo(::std::ostream* const os) const {
    *os << "is not fit::ok with value ";
    if constexpr (requires { expected_.DescribeNegationTo(os); }) {
      expected_.DescribeNegationTo(os);
    } else if constexpr (requires { expected_.impl().DescribeNegationTo(os); }) {
      expected_.impl().DescribeNegationTo(os);
    } else {
      ::testing::internal::UniversalPrinter<E>::Print(expected_, os);
    }
  }

 private:
  E expected_;
};

}  // namespace internal

template <typename E>
inline internal::SyscallSuccessMatcher<E> SyscallSucceedsWithValue(E expected) {
  return internal::SyscallSuccessMatcher<E>(::std::move(expected));
}

inline internal::SyscallSuccessMatcher<internal::AnySuccessValueMatcher> SyscallSucceeds() {
  return SyscallSucceedsWithValue(internal::AnySuccessValueMatcher());
}

inline ::testing::PolymorphicMatcher<internal::SyscallFailureMatcher> SyscallFailsWithErrno(
    ::testing::Matcher<int> expected) {
  return ::testing::MakePolymorphicMatcher(internal::SyscallFailureMatcher(::std::move(expected)));
}

// Overload taking an int so that SyscallFailsWithErrno(<specific errno>) uses
// internal::SpecificErrno (which stringifies the errno) rather than
// ::testing::Eq (which doesn't).
inline ::testing::PolymorphicMatcher<internal::SyscallFailureMatcher> SyscallFailsWithErrno(
    int const expected) {
  return SyscallFailsWithErrno(internal::SpecificErrno(expected));
}

inline ::testing::PolymorphicMatcher<internal::SyscallFailureMatcher> SyscallFails() {
  return SyscallFailsWithErrno(::testing::Gt(0));
}

inline ::testing::PolymorphicMatcher<internal::SyscallResultSuccessMatcher> SyscallResultIsOk() {
  return ::testing::MakePolymorphicMatcher(internal::SyscallResultSuccessMatcher());
}

template <typename E>
inline ::testing::PolymorphicMatcher<internal::SyscallResultSuccessWithValueMatcher<E>>
SyscallResultIsOkWithValue(E expected) {
  return ::testing::MakePolymorphicMatcher(
      internal::SyscallResultSuccessWithValueMatcher<E>(::std::move(expected)));
}

template <typename E>
inline ::testing::PolymorphicMatcher<internal::SyscallResultSuccessWithValueMatcher<E>>
SyscallResultIsOk(E expected) {
  return SyscallResultIsOkWithValue(::std::move(expected));
}

inline ::testing::PolymorphicMatcher<internal::SyscallResultFailureMatcher> SyscallResultIsErrno(
    ::testing::Matcher<int> expected) {
  return ::testing::MakePolymorphicMatcher(
      internal::SyscallResultFailureMatcher(std::move(expected)));
}

inline ::testing::PolymorphicMatcher<internal::SyscallResultFailureMatcher> SyscallResultIsErrno(
    int const expected) {
  return SyscallResultIsErrno(internal::SpecificErrno(expected));
}

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_SYSCALL_MATCHERS_H_
