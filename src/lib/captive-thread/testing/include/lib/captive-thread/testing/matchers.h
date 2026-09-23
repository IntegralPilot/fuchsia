// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_CAPTIVE_THREAD_TESTING_INCLUDE_LIB_CAPTIVE_THREAD_TESTING_MATCHERS_H_
#define SRC_LIB_CAPTIVE_THREAD_TESTING_INCLUDE_LIB_CAPTIVE_THREAD_TESTING_MATCHERS_H_

#include <lib/captive-thread/captive-thread.h>
#include <lib/captive-thread/registers.h>
#include <zircon/exception.h>

#include <concepts>
#include <format>
#include <ios>
#include <ostream>
#include <type_traits>

#include <gmock/gmock.h>

namespace captive_thread::testing {

using namespace std::string_literals;

// This is a helper used by some matchers below to match on values of type
// zx::result<CaptiveThread*> as returned by CaptiveThread::WaitForException().
inline CaptiveThread* MatcherThread(zx::result<CaptiveThread*> result, auto* result_listener) {
  if (result.is_ok()) {
    return *result;
  }
  *result_listener << "where thread not ready: " << result.status_string();
  return nullptr;
}

// This matches a CaptiveThread::WaitForException() return value when the
// thread has stopped in an exception.
MATCHER(GotException, "") {
  const CaptiveThread* thread = MatcherThread(arg, result_listener);
  if (thread && !thread->InException()) {
    *result_listener << ::testing::PrintToString(*thread);
  }
  return thread && thread->InException();
}

// With another matcher as argument, it matches an exception with whose
// zx_exception_report_t is matched by that other matcher.
MATCHER_P(GotException, matcher,
          (negation ? "not "s : ""s) + "in exception that "s +
              ::testing::DescribeMatcher<zx_exception_report_t>(matcher, negation)) {
  const CaptiveThread* thread = MatcherThread(arg, result_listener);
  if (thread && !thread->InException()) {
    *result_listener << ::testing::PrintToString(*thread);
  }
  return thread && thread->InException() &&
         ::testing::ExplainMatchResult(matcher, *thread->ExceptionReport(), result_listener);
}

// This is a helper used by some matchers below to match on values either of
// type zx_exception_report_t when the matcher is passed into GotException(),
// or of the type GotException() itself matches on, so as to implicitly wrap
// GotException() around that matcher.
constexpr const zx_exception_report_t* MatcherExceptionReport(  //
    const zx_exception_report_t& report, auto* result_listener) {
  return &report;
}

inline std::optional<zx_exception_report_t> MatcherExceptionReport(  //
    zx::result<CaptiveThread*> result, auto* result_listener) {
  std::optional<zx_exception_report_t> report;
  if (result.is_error()) {
    *result_listener << "where thread not ready: " << result.status_string();
  } else {
    report = result->ExceptionReport();
    if (!report) {
      *result_listener << "where thread is not in any exception";
    }
  }
  return report;
}

// This is a matcher on zx_exception_report_t that wraps a matcher on the
// zx_excp_type_t value.
MATCHER_P(WithType, matcher,
          (negation ? "not type "s : "type "s) +
              ::testing::DescribeMatcher<zx_excp_type_t>(matcher, negation)) {
  auto report = MatcherExceptionReport(arg, result_listener);
  if (!report) {
    return false;
  }
  const zx_excp_type_t excp_type = report->header.type;
  *result_listener << "where the type is " << zx_exception_get_string(excp_type) << " ("
                   << std::showbase << std::hex << excp_type << ")";
  return ::testing::ExplainMatchResult(matcher, excp_type, result_listener);
}

// This is a composite matcher used inside GotException() to match the
// canonical exception type induced by __builtin_trap() (and many other ways).
MATCHER(IsTrap, "") {
  return ::testing::ExplainMatchResult(WithType(kTrapException), arg, result_listener);
}

// This is a composite matcher used inside GotException() to match any page
// fault.
MATCHER(IsPageFault, "") {
  return ::testing::ExplainMatchResult(WithType(ZX_EXCP_FATAL_PAGE_FAULT), arg, result_listener);
}

// This is a composite matcher used inside GotException() to match a page fault
// whose fault address is matched by the argument matcher.
MATCHER_P(IsPageFault, matcher,
          (negation ? "not "s : ""s) + "page fault at address that "s +
              ::testing::DescribeMatcher<uint64_t>(matcher, negation)) {
  using ::testing::AllOf;
  using ::testing::ResultOf;
  auto report = MatcherExceptionReport(arg, result_listener);
  if (!report) {
    return false;
  }
  auto address_matcher = ResultOf("fault address", FaultAddress, matcher);
  return ::testing::ExplainMatchResult(  //
      AllOf(IsPageFault(), address_matcher), *report, result_listener);
}

MATCHER(IsSingleStep, "") {
  return ::testing::ExplainMatchResult(WithType(kSingleStepException), arg, result_listener);
}

constexpr auto GotSingleStep() { return GotException(IsSingleStep()); }

// HasRegisters() just checks that the registers can be fetched at all, which
// requires a stopped thread.  HasRegister<zx_thread_state_*_regs_t>() can
// specify which registers, the default being zx_thread_state_general_regs_t.
// With a matcher argument, that matcher is applied to the ..._regs_t type.

template <RegistersType Regs, class Matcher>
class HasRegistersMatcher {
 public:
  using is_gtest_matcher = void;

  constexpr explicit HasRegistersMatcher(Matcher matcher) : matcher_{std::move(matcher)} {}

  bool MatchAndExplain(auto&& arg, ::testing::MatchResultListener* result_listener) const {
    CaptiveThread* thread = MatcherThread(arg, result_listener);
    if (!thread) {
      return false;
    }
    return MatchAndExplain(thread->Registers<Regs>(), result_listener);
  }

  bool MatchAndExplain(zx::result<Regs> regs,
                       ::testing::MatchResultListener* result_listener) const {
    if (regs.is_error()) {
      *result_listener << "cannot get thread registers: " << regs.status_string();
      return false;
    }
    return ::testing::ExplainMatchResult(matcher_, *regs, result_listener);
  }

  void DescribeTo(std::ostream* os) const {
    *os << "has registers";
    if (kMatcher) {
      *os << " that " << ::testing::DescribeMatcher<Regs>(matcher_, false);
    }
  }

  void DescribeNegationTo(std::ostream* os) const {
    *os << "has no registers";
    if (kMatcher) {
      *os << " that ";
      ::testing::DescribeMatcher<Regs>(matcher_, false);
    }
  }

 private:
  static constexpr bool kMatcher = !std::same_as<Matcher, std::decay_t<decltype(::testing::_)>>;

  Matcher matcher_;
};

template <RegistersType Regs = zx_thread_state_general_regs_t, class Matcher>
constexpr auto HasRegisters(Matcher matcher) {
  return HasRegistersMatcher<Regs, Matcher>{std::move(matcher)};
}

template <RegistersType Regs = zx_thread_state_general_regs_t>
constexpr auto HasRegisters() {
  return HasRegisters(::testing::_);
}

// WithPc(m) matches zx_thread_state_general_regs_t with PC that m matches.
// It's used inside HasRegisters().  WithSp() and WithTp() are similar for the
// stack pointer and thread pointer, respectively.
MATCHER_P(WithPc, matcher, "") {
  return ::testing::ExplainMatchResult(matcher, SpecialRegisters(arg).pc(), result_listener);
}
MATCHER_P(WithSp, matcher, "") {
  return ::testing::ExplainMatchResult(matcher, SpecialRegisters(arg).sp(), result_listener);
}
MATCHER_P(WithFp, matcher, "") {
  return ::testing::ExplainMatchResult(matcher, SpecialRegisters(arg).fp(), result_listener);
}
MATCHER_P(WithReturnAddress, matcher, "") {
  std::optional<uint64_t> ra = SpecialRegisters(arg).ra();
  return !ra || ::testing::ExplainMatchResult(matcher, ra, result_listener);
}
MATCHER_P(WithScsp, matcher, "") {
  std::optional<uint64_t> scsp = SpecialRegisters(arg).scsp();
  return !scsp || ::testing::ExplainMatchResult(matcher, scsp, result_listener);
}
MATCHER_P(WithTp, matcher, "") {
  return ::testing::ExplainMatchResult(matcher, SpecialRegisters(arg).tp(), result_listener);
}
MATCHER_P(WithReturnValue, matcher, "") {
  return ::testing::ExplainMatchResult(matcher, ReturnValueRegisters(arg).front(), result_listener);
}

struct Hex {
  constexpr Hex() = default;

  constexpr explicit(false) Hex(uint64_t x) : value{x} {}

  constexpr explicit(false) operator uint64_t() const { return value; }

  constexpr auto operator<=>(const Hex&) const = default;

  constexpr std::string AsString() const { return std::format("{:#x}", value); }

  friend void PrintTo(const Hex& hex, std::ostream* os) { *os << hex.AsString(); }

  uint64_t value = 0;
};

constexpr std::ostream& operator<<(std::ostream& os, const Hex& hex) {
  return os << hex.AsString();
}

constexpr Hex AsHex(uint64_t value) { return Hex{value}; }

// HasRegisters(AsContainer(...)) can be used to apply to a container of
// std::pair<std::string, uint64_t> with the register names and values in the
// struct order, for more convenient matching and output.
using RegistersContainer = std::vector<std::pair<std::string, Hex>>;
RegistersContainer RegistersAsContainer(const zx_thread_state_general_regs_t&);
RegistersContainer RegistersAsContainer(const zx_thread_state_fp_regs_t&);
RegistersContainer RegistersAsContainer(const zx_thread_state_vector_regs_t&);
RegistersContainer RegistersAsContainer(const zx_thread_state_debug_regs_t&);
template <RegistersType Regs = zx_thread_state_general_regs_t>
inline RegistersContainer RegistersAsContainer(CaptiveThread& thread) {
  auto regs = thread.Registers<Regs>();
  if (regs.is_error()) {
    return {};
  }
  return RegistersAsContainer(*regs);
}
MATCHER_P(AsContainer, matcher,
          (negation ? "don't "s : ""s) + "match as a container that "s +
              ::testing::DescribeMatcher<RegistersContainer>(matcher, negation)) {
  return ::testing::ExplainMatchResult(matcher, RegistersAsContainer(arg), result_listener);
}

void PrintTo(const RegistersContainer&, std::ostream*);

constexpr std::ostream& operator<<(std::ostream& os, const RegistersContainer& regs) {
  PrintTo(regs, &os);
  return os;
}

}  // namespace captive_thread::testing

#endif  // SRC_LIB_CAPTIVE_THREAD_TESTING_INCLUDE_LIB_CAPTIVE_THREAD_TESTING_MATCHERS_H_
