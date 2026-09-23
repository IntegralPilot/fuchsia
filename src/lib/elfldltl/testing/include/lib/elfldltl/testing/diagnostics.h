// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_DIAGNOSTICS_H_
#define SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_DIAGNOSTICS_H_

#include <lib/elfldltl/diagnostics-ostream.h>
#include <lib/elfldltl/diagnostics.h>

#include <source_location>
#include <sstream>
#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>

#ifdef USE_ZXTEST
#include <zxtest/zxtest.h>
#else
#include <gtest/gtest.h>
#endif

namespace elfldltl::testing {

// Diagnostic flags for signaling as much information as possible.
struct TestingDiagnosticsFlags {
  template <auto>
  struct UniqueTrueType : public std::true_type {};

  [[no_unique_address]] UniqueTrueType<0> multiple_errors;
  [[no_unique_address]] UniqueTrueType<1> warnings_are_errors;
  [[no_unique_address]] UniqueTrueType<2> extra_checking;
};

// This never does anything, but always instantiates calls (never reached at
// runtime) to the PrintfDiagnosticsReport and OstreamDiagnosticsReport
// functions so it tests that the argument types are accepted by those at
// compile time.
class ReportTypeCheck {
 public:
  template <typename... Args>
  void operator()(Args&&... args) const {
    if (do_printf()) {
      constexpr auto test_printf = [](const char* fmt, ...) { return -1; };
      PrintfDiagnosticsReport(test_printf)(std::forward<Args>(args)...);
    } else if (do_ostream()) {
      std::stringstream os;
      OstreamDiagnosticsReport(os)(std::forward<Args>(args)...);
    }
  }

  // This can guard calls so they can use std::forward twice in the source
  // without actually forwarding any value twice.
  static bool guard_call() { return false; }

 private:
  static bool do_printf() { return false; }
  static bool do_ostream() { return false; }
};

struct ReportToString {
  template <typename... Args>
  inline std::string operator()(Args&&... args) const {
    std::stringstream os;
    auto report = OstreamDiagnosticsReport(os);
    report(std::forward<decltype(args)>(args)...);
    return std::move(os).str();
  }
};

// This is a Report callable object that causes a gtest failure if called.
class ExpectNoReport {
 public:
  explicit constexpr ExpectNoReport(std::source_location location = std::source_location::current())
      : location_{location} {}

  template <typename... Args>
  constexpr bool operator()(Args&&... args) const {
    ADD_FAILURE() << location_.file_name() << ":" << location_.line() << ":" << location_.column()
                  << ": in " << location_.function_name() << ": Expected no diagnostics, not: "
                  << ReportToString{}(std::forward<Args>(args)...);
    if (ReportTypeCheck::guard_call()) {
      ReportTypeCheck{}(std::forward<Args>(args)...);
    }
    return true;
  }

 private:
  std::source_location location_;
};

// This is a Report callable object that checks for an expected sequence of
// arguments: calling with different arguments causes a gtest failure.
template <typename... Args>
class ExpectReport {
 public:
  static_assert(sizeof...(Args) > 0,
                "Expecting an empty .FormatError() call is invalid."
                " Use elfldltl::testing::ExpectOkDiagnostics instead.");

  static constexpr size_t kExpectedArgumentCount = sizeof...(Args);

  ExpectReport() = delete;

  constexpr ExpectReport(const ExpectReport& other) = delete;

  constexpr ExpectReport(ExpectReport&& other) noexcept
      : expected_{std::move(other.expected_)},
        location_{other.location_},
        state_{std::exchange(other.state_, State::kMoved)} {
    EXPECT_NE(state_, State::kMoved);
  }

  explicit constexpr ExpectReport(Args... args,
                                  std::source_location location = std::source_location::current())
      : expected_{std::forward<Args>(args)...}, location_{location} {}

  template <typename... Ts>
  constexpr bool operator()(Ts&&... args) {
    if (ReportTypeCheck::guard_call()) {
      ReportTypeCheck{}(std::forward<Ts>(args)...);
    }
    constexpr size_t called_argument_count = sizeof...(Ts);
    switch (state_) {
      case State::kMoved:
      case State::kCalled:
        // This produces different failure messages when in these states.
        Diagnose(std::forward<Ts>(args)...);
        break;
      case State::kUncalled:
        if constexpr (called_argument_count != kExpectedArgumentCount) {
          EXPECT_EQ(called_argument_count, kExpectedArgumentCount)
              << "wrong number of arguments in Diagnostics error call";
          Diagnose(std::forward<Ts>(args)...);
        } else if (!Check(kSeq, args...)) {
          Diagnose(std::forward<Ts>(args)...);
        }
        break;
    }
    state_ = State::kCalled;  // Changes Diagnose behavior for next time.
    return true;              // Always ask to keep reporting more errors.
  }

  void ExpectCalledOrMoved() const {
    if (state_ == State::kUncalled) {
      ADD_FAILURE() << "Missing expected error: " << std::apply(ReportToString{}, expected_);
    }
  }

  // This puts the report into "moved-from" state, where ExpectCalledOrMoved()
  // will not trigger a gtest failure.
  void Release() && { state_ = State::kMoved; }

  template <typename Report>
  bool ReportTo(Report&& report) const {
    return std::apply(std::forward<Report>(report), expected_);
  }

 private:
  enum class State { kUncalled, kCalled, kMoved };

  template <typename T, typename T2 = void>
  struct NormalizeImpl {
    using type = T;
  };

  template <typename T>
  struct NormalizeImpl<T, std::enable_if_t<std::is_constructible_v<std::string_view, T>>> {
    using type = std::string_view;
  };

  template <typename T>
  struct NormalizeImpl<T, std::enable_if_t<std::is_integral_v<T>>> {
    using type = uint64_t;
  };

  template <typename T>
  using Normalize = typename NormalizeImpl<std::decay_t<T>>::type;

  using ExpectedArgs = std::tuple<Normalize<Args>...>;

  template <size_t I>
  using CheckType = std::tuple_element_t<I, ExpectedArgs>;

  static constexpr auto kSeq = std::make_index_sequence<sizeof...(Args)>();

  template <size_t... I, typename... Ts>
  [[nodiscard]] bool Check(std::index_sequence<I...> seq, Ts... args) const {
    return (CheckOne<I>(std::get<I>(expected_), std::forward<Ts>(args))
            // Use & instead of && to avoid short-circuiting in the fold:
            // Check and diagnose all arguments, not just the first mismatch.
            & ...);
  }

  template <size_t I, typename T>
  [[nodiscard]] static int CheckOne(CheckType<I> expected, T arg) {
    using CT = CheckType<I>;
    if constexpr (std::is_convertible_v<T, CT>) {
      CT normalized_arg = static_cast<CT>(std::move(arg));
      EXPECT_EQ(normalized_arg, expected) << "argument " << I << " of " << kExpectedArgumentCount;
      return normalized_arg == expected;
    }
    ADD_FAILURE() << "incompatible types for argument " << I << " of " << kExpectedArgumentCount
                  << " " << __PRETTY_FUNCTION__;
    return false;
  }

  template <typename... Ts>
  void Diagnose(Ts&&... args) const {
    std::string formatted_arguments = ReportToString{}(std::forward<Ts>(args)...);
    std::string formatted_expected_arguments = std::apply(ReportToString{}, expected_);
    switch (state_) {
      case State::kMoved:
        ADD_FAILURE() << "Diagnostics used after std::move'd from!"
                      << "\nFor expected error: " << formatted_expected_arguments << "\n"
                      << location_.file_name() << ":" << location_.line() << ":"
                      << location_.column() << ": in " << location_.function_name()
                      << "\nCalled with: " << formatted_arguments;
        break;
      case State::kCalled:
        ADD_FAILURE() << "Expected only one error: " << formatted_expected_arguments
                      << location_.file_name() << ":" << location_.line() << ":"
                      << location_.column() << ": in " << location_.function_name()
                      << "\nBut also got: " << formatted_arguments;
        break;
      case State::kUncalled:
        EXPECT_NE(formatted_arguments, formatted_expected_arguments)
            << "Diagnose with identical strings??\n"
            << location_.file_name() << ":" << location_.line() << ":" << location_.column()
            << ": in " << location_.function_name();
        EXPECT_EQ(formatted_arguments, formatted_expected_arguments)
            << "Expected different Diagnostics arguments\n"
            << location_.file_name() << ":" << location_.line() << ":" << location_.column()
            << ": in " << location_.function_name();
        break;
    }
  }

  ExpectedArgs expected_;
  std::source_location location_;
  State state_ = State::kUncalled;
};

// Deduction guide.
template <typename... Args>
ExpectReport(Args&&...) -> ExpectReport<Args...>;

// This Diagnostics object is constructed with expected error arguments.  If
// it's called with different arguments or not called exactly once before it
// goes out of scope, it causes a gtest failure.
template <typename... Args>
class ExpectedSingleError : public Diagnostics<ExpectReport<Args...>, TestingDiagnosticsFlags> {
 public:
  using ExpectedReport = ExpectReport<Args...>;
  using ExpectedDiagnostics = Diagnostics<ExpectedReport, TestingDiagnosticsFlags>;

  ExpectedSingleError(const ExpectedSingleError&) = delete;

  constexpr ExpectedSingleError(ExpectedSingleError&&) = default;

  explicit constexpr ExpectedSingleError(Args... args)
      : ExpectedDiagnostics{ExpectedReport{std::forward<Args>(args)...}} {}

  ~ExpectedSingleError() { this->report().ExpectCalledOrMoved(); }
};

// Deduction guide.
template <typename... Args>
ExpectedSingleError(Args&&...) -> ExpectedSingleError<Args...>;

// This Diagnostics object causes a gtest failure every time it's called.
struct ExpectOkDiagnostics : public Diagnostics<ExpectNoReport, TestingDiagnosticsFlags> {
  explicit constexpr ExpectOkDiagnostics(
      std::source_location location = std::source_location::current())
      : Diagnostics<ExpectNoReport, TestingDiagnosticsFlags>{ExpectNoReport{location}} {}

  constexpr ExpectOkDiagnostics(const ExpectOkDiagnostics&) = default;
};

template <typename Diagnostics>
constexpr bool IsExpectOkDiagnostics(Diagnostics& diag) {
  static_assert(!std::is_const_v<Diagnostics>,
                "Diagnostics reference should be propagated without adding qualifiers");
  return std::is_same_v<Diagnostics, ExpectOkDiagnostics>;
}

// This collects a list of ExpectReport objects and applies consecutive calls
// to each in turn.  If it doesn't get a matching sequence of calls in order
// before it goes out of scope, it causes gtest failures.
template <class... Reports>
class ExpectReportList {
 public:
  static_assert(sizeof...(Reports) > 0,
                "elfldltl::testing::ExpectedErrorList invalid with no errors."
                " Use elfldltl::testing::ExpectOkDiagnostics instead.");

  ExpectReportList() = delete;

  ExpectReportList(const ExpectReportList&) = delete;

  constexpr ExpectReportList(ExpectReportList&& other)
      : reports_{std::move(other.reports_)},
        location_{other.location_},
        next_{std::exchange(other.next_, kCount)} {}

  ExpectReportList& operator=(const ExpectReportList&) = delete;

  explicit constexpr ExpectReportList(
      Reports... reports, std::source_location location = std::source_location::current())
      : reports_{std::move(reports)...}, location_{location} {}

  template <typename... Args>
  constexpr bool operator()(Args&&... args) {
    if (ReportTypeCheck::guard_call()) {
      ReportTypeCheck{}(std::forward<Args>(args)...);
    }
    EXPECT_LT(next_, kCount) << location_.file_name() << ":" << location_.line() << ":"
                             << location_.column() << ": in " << location_.function_name()
                             << ": too many errors, next error unexpected...";
    return next_ < kCount  //
               ? ExpectError(kSeq, next_++, std::forward<Args>(args)...)
               : ExpectNoReport{location_}(std::forward<Args>(args)...);
  }

  // This puts the report list into "moved-from" state where no gtest
  // expectations will be asserted at destruction.
  void Release() && {
    auto release = [](auto&... report) { (std::move(report).Release(), ...); };
    std::apply(release, reports_);
    next_ = kCount;
  }

  ~ExpectReportList() {
    EXPECT_EQ(next_, kCount) << "wrong number of errors";
    auto expect_called = [](auto&... report) { (report.ExpectCalledOrMoved(), ...); };
    std::apply(expect_called, reports_);
  }

  template <typename Report>
  bool ReportTo(Report&& report) const {
    auto one_report = [&report](auto& one) { return one.ReportTo(report); };
    return std::apply(one_report, reports_);
  }

 private:
  static constexpr size_t kCount = sizeof...(Reports);
  static constexpr auto kSeq = std::make_index_sequence<kCount>();

  template <size_t... I, class... Args>
  constexpr bool ExpectError(std::index_sequence<I...> seq, size_t idx, Args&&... args) {
    return ((idx == I && std::get<I>(reports_)(std::forward<Args>(args)...)) || ...);
  }

  std::tuple<Reports...> reports_;
  std::source_location location_;
  size_t next_ = 0;
};

// Deduction guide.
template <class... Reports>
ExpectReportList(Reports&&...) -> ExpectReportList<std::decay_t<Reports>...>;

template <class... Reports>
struct ExpectedErrorList
    : public Diagnostics<ExpectReportList<Reports...>, TestingDiagnosticsFlags> {
  using ReportList = ExpectReportList<Reports...>;

  explicit constexpr ExpectedErrorList(
      Reports... reports, std::source_location location = std::source_location::current())
      : Diagnostics<ReportList, TestingDiagnosticsFlags>{
            ReportList{std::move(reports)..., location}} {}

  void Release() && { std::move(this->report()).Release(); }
};

// Deduction guide.
template <class... Reports>
ExpectedErrorList(Reports&&...) -> ExpectedErrorList<std::decay_t<Reports>...>;

}  // namespace elfldltl::testing

#endif  // SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_DIAGNOSTICS_H_
