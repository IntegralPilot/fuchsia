// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <zircon/availability.h>

// This file tests the macros defined in <zircon/availability.h> at
// __Fuchsia_API_level__. To "run the test," compile it at a variety of API
// levels, including numbered API levels and named API levels such as PLATFORM.

// The tests use levels that are not supported in production. Define the necessary macros.
#define FUCHSIA_INTERNAL_LEVEL_10000_() 10000
#define FUCHSIA_INTERNAL_LEVEL_2147483648_() 2147483648
#define FUCHSIA_INTERNAL_LEVEL_4294967296_() 4294967296

// =============================================================================
// ZX_*_SINCE() macro tests.
//
// The following tests both the macro mechanics, including support for named levels such as `HEAD`,
// and calling functions annotated with the macros in cases that won't cause errors. They cannot
// test that deprecation warnings or compile errors are emitted.
// =============================================================================

void AddedAtLevel15(void) ZX_AVAILABLE_SINCE(15) {}
void AddedAtNEXT(void) ZX_AVAILABLE_SINCE(NEXT) {}
void AddedAtHEAD(void) ZX_AVAILABLE_SINCE(HEAD) {}

void DeprecatedAtLevel15(void) ZX_DEPRECATED_SINCE(12, 15, "Use AddedAtLevel15().") {}
void DeprecatedAtNEXT(void) ZX_DEPRECATED_SINCE(15, NEXT, "Use AddedAtNEXT().") {}
void DeprecatedAtHEAD(void) ZX_DEPRECATED_SINCE(15, HEAD, "Use AddedAtHEAD().") {}

void RemovedAtLevel21(void) ZX_REMOVED_SINCE(12, 15, 21, "Use AddedAtLevel15().") {}
void RemovedAtNEXT(void) ZX_REMOVED_SINCE(15, 10000, NEXT, "Use AddedAtLevel10000().") {}
void RemovedAtHEAD(void) ZX_REMOVED_SINCE(15, NEXT, HEAD, "Use AddedAtLevelNEXT().") {}

void CallVersionedFunctions(void) {
#pragma clang diagnostic push
// Ensure deprecation warnings are treated as errors so they would cause the test to fail.
// This applies to the entire test unless temporarily overridden.
#pragma clang diagnostic error "-Wdeprecated-declarations"

  AddedAtLevel15();
#if defined(BUILT_AT_NUMBERED_API_LEVEL)
  DeprecatedAtNEXT();
  DeprecatedAtHEAD();
  RemovedAtNEXT();
  RemovedAtHEAD();
#else
  AddedAtNEXT();
#endif

  // Deprecation warnings that would occur must be suppressed to avoid failing the build..
  // The warnings must always be suppressed for DeprecatedAtLevel15(), but only need to be
  // suppressed for DeprecatedAtHEAD() at HEAD and higher because no warning should be produced at
  // lower API levels.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  DeprecatedAtLevel15();
#pragma clang diagnostic pop

#pragma clang diagnostic push
#if !defined(BUILT_AT_NUMBERED_API_LEVEL)
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
#endif
  DeprecatedAtHEAD();
#pragma clang diagnostic pop

  // RemovedAtLevel15() cannot be called at any level at which this test is run.
  // RemovedAtHEAD() can be called at any level below HEAD.
#pragma clang diagnostic push
#if defined(BUILT_AT_NUMBERED_API_LEVEL)
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  RemovedAtHEAD();
#endif
#pragma clang diagnostic pop

#pragma clang diagnostic pop
}

// =============================================================================
// FUCHSIA_API_LEVEL_*() macro tests.
// =============================================================================

// Test all named special levels
#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT) && FUCHSIA_API_LEVEL_AT_LEAST(HEAD) && \
    FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
static_assert(__Fuchsia_API_level__ == 4293918720, "Condition must be true for PLATFORM");
#else
static_assert(__Fuchsia_API_level__ <= 4292870144, "Condition can only be true for PLATFORM");
#endif

// 0x80000000
#define FIRST_RESERVED_API_LEVEL() 2147483648
// 0xFFFFFFFF + 1
#define UINT32_MAX_PLUS_ONE() 4294967296

// Test using conditions that are always true.

#if !FUCHSIA_API_LEVEL_AT_LEAST(9)
#error API level should be greater than 9.
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(UINT32_MAX_PLUS_ONE())
#error API levels are 32 bits.
#endif

#if FUCHSIA_API_LEVEL_LESS_THAN(9)
#error API level should not be less than 9.
#endif

#if !FUCHSIA_API_LEVEL_LESS_THAN(UINT32_MAX_PLUS_ONE())
#error API levels are 32 bits.
#endif

#if FUCHSIA_API_LEVEL_AT_MOST(9)
#error API level should not be 9 or less.
#endif

#if !FUCHSIA_API_LEVEL_AT_MOST(PLATFORM)
#error API level cannot be greater than PLATFORM.
#endif

#if !FUCHSIA_API_LEVEL_AT_MOST(UINT32_MAX_PLUS_ONE())
#error API levels are 32 bits.
#endif

// Test using cases have different results depending on whether the target API
// level is numbered or special.
#if defined(BUILT_AT_NUMBERED_API_LEVEL)

static_assert(!FUCHSIA_API_LEVEL_AT_LEAST(10000), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_LESS_THAN(10000), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_AT_MOST(10000), "Unexpected result");

static_assert(!FUCHSIA_API_LEVEL_AT_LEAST(FIRST_RESERVED_API_LEVEL()), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_LESS_THAN(FIRST_RESERVED_API_LEVEL()), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_AT_MOST(FIRST_RESERVED_API_LEVEL()), "Unexpected result");

static_assert(!FUCHSIA_API_LEVEL_AT_LEAST(NEXT), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_LESS_THAN(NEXT), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_AT_MOST(NEXT), "Unexpected result");

static_assert(!FUCHSIA_API_LEVEL_AT_LEAST(HEAD), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_LESS_THAN(HEAD), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_AT_MOST(HEAD), "Unexpected result");

static_assert(!FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_LESS_THAN(PLATFORM), "Unexpected result");
static_assert(FUCHSIA_API_LEVEL_AT_MOST(PLATFORM), "Unexpected result");

#else

static_assert(FUCHSIA_API_LEVEL_AT_LEAST(10000), "Unexpected result");
static_assert(!FUCHSIA_API_LEVEL_LESS_THAN(10000), "Unexpected result");
static_assert(!FUCHSIA_API_LEVEL_AT_MOST(10000), "Unexpected result");

static_assert(FUCHSIA_API_LEVEL_AT_LEAST(FIRST_RESERVED_API_LEVEL()), "Unexpected result");
static_assert(!FUCHSIA_API_LEVEL_LESS_THAN(FIRST_RESERVED_API_LEVEL()), "Unexpected result");
static_assert(!FUCHSIA_API_LEVEL_AT_MOST(FIRST_RESERVED_API_LEVEL()), "Unexpected result");

static_assert(FUCHSIA_API_LEVEL_AT_LEAST(NEXT), "Unexpected result");

static_assert(FUCHSIA_API_LEVEL_AT_MOST(PLATFORM), "Unexpected result");

#endif  // defined(BUILT_AT_NUMBERED_API_LEVEL)

#undef FIRST_RESERVED_API_LEVEL
#undef UINT32_MAX_PLUS_ONE
#undef FUCHSIA_INTERNAL_LEVEL_10000_
#undef FUCHSIA_INTERNAL_LEVEL_2147483648_
#undef FUCHSIA_INTERNAL_LEVEL_4294967296_

// When targeting a numbered API level, we can test the macros at and around it.
#if defined(BUILT_AT_NUMBERED_API_LEVEL)

// Verify the preprocessor values defined by the build file.
// Since the macros only accept literals, all relative levels must be provided as literals via
// additional preprocessor values defined by the build.
#if !defined(__Fuchsia_API_level__) || __Fuchsia_API_level__ != BUILT_AT_NUMBERED_API_LEVEL
#error BUILT_AT_NUMBERED_API_LEVEL does not match __Fuchsia_API_level__.
#endif
#if BUILT_AT_NUMBERED_API_LEVEL_MINUS_ONE != (BUILT_AT_NUMBERED_API_LEVEL - 1)
#error BUILT_AT_NUMBERED_API_LEVEL_MINUS_ONE is not correctly defined.
#endif
#if BUILT_AT_NUMBERED_API_LEVEL_PLUS_ONE != (BUILT_AT_NUMBERED_API_LEVEL + 1)
#error BUILT_AT_NUMBERED_API_LEVEL_PLUS_ONE is not correctly defined.
#endif

#if !FUCHSIA_API_LEVEL_AT_LEAST(BUILT_AT_NUMBERED_API_LEVEL)
#error API level should be at least `BUILT_AT_NUMBERED_API_LEVEL`.
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(BUILT_AT_NUMBERED_API_LEVEL_PLUS_ONE)
#error API level should be less than `BUILT_AT_NUMBERED_API_LEVEL + 1`.
#endif

#if FUCHSIA_API_LEVEL_LESS_THAN(BUILT_AT_NUMBERED_API_LEVEL)
#error API level should not be less than `BUILT_AT_NUMBERED_API_LEVEL`.
#endif

#if !FUCHSIA_API_LEVEL_LESS_THAN(BUILT_AT_NUMBERED_API_LEVEL_PLUS_ONE)
#error API level should be less than `BUILT_AT_NUMBERED_API_LEVEL + 1`.
#endif

#if FUCHSIA_API_LEVEL_AT_MOST(BUILT_AT_NUMBERED_API_LEVEL_MINUS_ONE)
#error API level should greater than `BUILT_AT_NUMBERED_API_LEVEL - 1`.
#endif

#if !FUCHSIA_API_LEVEL_AT_MOST(BUILT_AT_NUMBERED_API_LEVEL)
#error API level should be no greater than `BUILT_AT_NUMBERED_API_LEVEL`.
#endif

#endif  // defined(BUILT_AT_NUMBERED_API_LEVEL)
