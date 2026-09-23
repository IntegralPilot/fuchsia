// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <zxtest/zxtest.h>

extern uint32_t EchoValue(uint32_t val);

// Asserts whose predicate evaluates to `true` should not terminate the process.
// In the case of debug asserts, we should always pass; either debug asserts are
// enabled and the predicate is true, or asserts are disabled and the statement
// should disappear.
TEST(ZxAssertCTest, ZxAssertValidPredicatePasses) {
  const uint32_t kExpected = __LINE__;
  ZX_ASSERT(EchoValue(kExpected) == kExpected);
}

TEST(ZxAssertCTest, ZxAssertMsgValidPredicatePasses) {
  const uint32_t kExpected = __LINE__;
  const uint32_t val = EchoValue(kExpected);
  ZX_ASSERT_MSG(val == kExpected, "Expected %u, Got %u; ", kExpected, val);
}

TEST(ZxAssertCTest, ZxDebugAssertValidPredicatePasses) {
  __UNUSED const uint32_t kExpected = __LINE__;
  ZX_DEBUG_ASSERT(EchoValue(kExpected) == kExpected);
}

TEST(ZxAssertCTest, ZxDebugAssertMsgValidPredicatePasses) {
  __UNUSED const uint32_t kExpected = __LINE__;
  __UNUSED const uint32_t val = EchoValue(kExpected);
  ZX_DEBUG_ASSERT_MSG(val == kExpected, "Expected %u, Got %u; ", kExpected, val);
}

static void ZxAssertInvalidPredicateFailsImpl(void) {
  const uint32_t kExpected = __LINE__;
  const uint32_t kActual = kExpected + 1;
  ZX_ASSERT(EchoValue(kActual) == kExpected);
}

TEST(ZxAssertCTest, ZxAssertInvalidPredicateFails) {
  ASSERT_DEATH(ZxAssertInvalidPredicateFailsImpl);
}

static void ZxAssertMsgInvalidPredicateFailsImpl(void) {
  const uint32_t kExpected = __LINE__;
  const uint32_t kActual = kExpected + 1;
  const uint32_t val = EchoValue(kActual);
  ZX_ASSERT_MSG(val == kExpected, "Expected %u, Got %u; ", kExpected, val);
}

TEST(ZxAssertCTest, ZxAssertMsgInvalidPredicateFails) {
  ASSERT_DEATH(ZxAssertMsgInvalidPredicateFailsImpl);
}

static void ZxDebugAssertInvalidPredicateFailsImpl(void) {
  const uint32_t kExpected = __LINE__;
  const uint32_t kActual = kExpected + 1;
  ZX_DEBUG_ASSERT(EchoValue(kActual) == kExpected);
}

TEST(ZxAssertCTest, ZxDebugAssertInvalidPredicateFails) {
#if ZX_DEBUG_ASSERT_IMPLEMENTED
  ASSERT_DEATH(ZxDebugAssertInvalidPredicateFailsImpl);
#else
  ZxDebugAssertInvalidPredicateFailsImpl();
#endif
}

static void ZxDebugAssertMsgInvalidPredicateFailsImpl(void) {
  const uint32_t kExpected = __LINE__;
  const uint32_t kActual = kExpected + 1;
  const uint32_t val = EchoValue(kActual);
  ZX_DEBUG_ASSERT_MSG(val == kExpected, "Expected %u, Got %u; ", kExpected, val);
}

TEST(ZxAssertCTest, ZxDebugAssertMsgInvalidPredicateFails) {
#if ZX_DEBUG_ASSERT_IMPLEMENTED
  ASSERT_DEATH(ZxDebugAssertMsgInvalidPredicateFailsImpl);
#else
  ZxDebugAssertMsgInvalidPredicateFailsImpl();
#endif
}
