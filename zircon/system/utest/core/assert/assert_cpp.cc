// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/assert.h>

#include <zxtest/zxtest.h>

extern "C" uint32_t EchoValue(uint32_t val);

namespace {

// Asserts whose predicate evaluates to `true` should not terminate the process.
// In the case of debug asserts, we should always pass; either debug asserts are
// enabled and the predicate is true, or asserts are disabled and the statement
// should disappear.
TEST(ZxAssertCppTest, ZxAssertValidPredicatePasses) {
  constexpr uint32_t kExpected = __LINE__;
  ZX_ASSERT(EchoValue(kExpected) == kExpected);
}

TEST(ZxAssertCppTest, ZxAssertMsgValidPredicatePasses) {
  constexpr uint32_t kExpected = __LINE__;

  // Note; this ASSERT test (and the related forms below) serve double duty.  In
  // addition to checking their related assert, they also make sure that the
  // assert implementation supports the new C++17 `if` syntax which allows
  // variables and their initialization to be introduced into the limited scoped
  // of the `if/else` statement.
  ZX_ASSERT_MSG(const uint32_t val = EchoValue(kExpected);
                val == kExpected, "Expected %u, Got %u; ", kExpected, val);
}

TEST(ZxAssertCppTest, ZxDebugAssertValidPredicatePasses) {
  constexpr uint32_t kExpected = __LINE__;
  ZX_DEBUG_ASSERT(EchoValue(kExpected) == kExpected);
}

TEST(ZxAssertCppTest, ZxDebugAssertMsgValidPredicatePasses) {
  constexpr uint32_t kExpected = __LINE__;
  ZX_DEBUG_ASSERT_MSG(const uint32_t val = EchoValue(kExpected);
                      val == kExpected, "Expected %u, Got %u; ", kExpected, val);
}

// Asserts whose predicate evaluates to `false` should fail and terminate the
// program (we expect program death).  Test this using the test framework's
// ASSERT_DEATH macro. In the case of debug asserts, we expect to program to die
// iff debug asserts are enabled, otherwise we should be able to make the same
// assert (with a false predicate) and still have the program survive.
TEST(ZxAssertCppTest, ZxAssertInvalidPredicateFails) {
  constexpr uint32_t kExpected = __LINE__;
  constexpr uint32_t kActual = kExpected + 1;
  ASSERT_DEATH([]() { ZX_ASSERT(EchoValue(kActual) == kExpected); });
}

TEST(ZxAssertCppTest, ZxAssertMsgInvalidPredicateFails) {
  constexpr uint32_t kExpected = __LINE__;
  constexpr uint32_t kActual = kExpected + 1;
  ASSERT_DEATH([]() {
    ZX_ASSERT_MSG(const uint32_t val = EchoValue(kActual);
                  val == kExpected, "Expected %u, Got %u; ", kExpected, val);
  });
}

TEST(ZxAssertCppTest, ZxDebugAssertInvalidPredicateFails) {
  constexpr uint32_t kExpected = __LINE__;
  constexpr uint32_t kActual = kExpected + 1;

#if ZX_DEBUG_ASSERT_IMPLEMENTED
  ASSERT_DEATH([]() { ZX_DEBUG_ASSERT(EchoValue(kActual) == kExpected); });
#else
  ZX_DEBUG_ASSERT(EchoValue(kActual) == kExpected);
#endif
}

TEST(ZxAssertCppTest, ZxDebugAssertMsgInvalidPredicateFails) {
  constexpr uint32_t kExpected = __LINE__;
  constexpr uint32_t kActual = kExpected + 1;
#if ZX_DEBUG_ASSERT_IMPLEMENTED
  ASSERT_DEATH([]() {
    ZX_DEBUG_ASSERT_MSG(const uint32_t val = EchoValue(kActual);
                        val == kExpected, "Expected %u, Got %u; ", kExpected, val);
  });
#else
  ZX_DEBUG_ASSERT_MSG(const uint32_t val = EchoValue(kActual);
                      val == kExpected, "Expected %u, Got %u; ", kExpected, val);
#endif
}

// There is really no good way to write negative compile-time tests, but we can
// (at least) have a few tests which can be manually enabled and run by someone
// at the time they modify assert.h
[[maybe_unused]] void NegativeCompilationChecks() {
  [[maybe_unused]] uint32_t val{0};

  // Assignment without extra ()s during a ZX_ASSERT should fail to compile.
#if TEST_WILL_NOT_COMPILE || 0
  ZX_ASSERT(val = 5);
#endif

  // Assignment without extra ()s during a ZX_ASSERT_MSG should fail to compile.
#if TEST_WILL_NOT_COMPILE || 0
  ZX_ASSERT_MSG(val = 5, "Should not compile");
#endif

  // Assignment without extra ()s during a ZX_DEBUG_ASSERT should fail to compile.
#if TEST_WILL_NOT_COMPILE || 0
  ZX_DEBUG_ASSERT(val = 5);
#endif

  // Assignment without extra ()s during a ZX_DEBUG_ASSERT_MSG should fail to compile.
#if TEST_WILL_NOT_COMPILE || 0
  ZX_DEBUG_ASSERT_MSG(val = 5, "Should not compile");
#endif
}

}  // namespace
