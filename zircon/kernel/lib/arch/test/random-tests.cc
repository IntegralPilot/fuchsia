// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/random.h>
#include <lib/unittest/unittest.h>

#include <ktl/optional.h>
#include <ktl/type_traits.h>

#include <ktl/enforce.h>

namespace {

// If supported, this test validates that arch::Random<Reseeded>::Get()
// yields a value *eventually*. We loop indefinitely, as the hardware should
// not be entropy-starved forever. If this test results in a timeout, then
// that gives us a signal that there is something fundamentally wrong with the
// underlying hardware or firmware.
template <bool Reseeded>
bool ArchRandomTest() {
  BEGIN_TEST;

  bool supported = arch::Random<Reseeded>::Supported();
  EXPECT_EQ(supported, !!supported);

  // TODO(https://fxbug.dev/546179341): Re-enable on ARM once entropy starvation
  // issues on arm64 builders are resolved.
#if defined(__arm__) || defined(__aarch64__)
  supported = false;
#endif

  if (supported) {
    // Note that Get() has its own internal retry logic.
    ktl::optional<uint64_t> result = arch::Random<Reseeded>::Get();
    while (!result) {
      printf(
          "WARNING: Failed to generate a random number after retrying "
          "a presumed sensible number of times. RNG is entropy-starved?\n");
      result = arch::Random<Reseeded>::Get();
    }
  }

  END_TEST;
}

bool PlainRandomTest() { return ArchRandomTest<false>(); }

bool ReseededRandomTest() { return ArchRandomTest<true>(); }

}  // namespace

UNITTEST_START_TESTCASE(ArchRandomTests)
UNITTEST("hardware RNG", PlainRandomTest)
UNITTEST("hardware reseeded RNG", ReseededRandomTest)
UNITTEST_END_TESTCASE(ArchRandomTests, "arch-random", "hardware RNG tests")
