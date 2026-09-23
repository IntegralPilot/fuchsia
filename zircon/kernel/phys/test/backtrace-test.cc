// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/zbi-format/zbi.h>
#include <stddef.h>
#include <stdio.h>
#include <zircon/assert.h>

#include <ktl/iterator.h>
#include <phys/stack.h>
#include <phys/symbolize.h>

#include "test-main.h"

namespace {

// BackTrace() omits its immediate caller, so Collect* itself won't appear.

[[gnu::noinline]] auto CollectFp() { return Symbolize::FramePointerBacktrace::BackTrace(); }

[[gnu::noinline]] auto CollectScs() { return boot_shadow_call_stack.BackTrace(); }

[[gnu::noinline]] PHYS_SINGLETHREAD ptrdiff_t Find() {
  constexpr auto bt_depth = [](auto&& bt) { return ktl::distance(bt.begin(), bt.end()); };

  printf("Collecting backtraces...\n");
  gSymbolize->Context();

  const auto fp_bt = CollectFp();
  const ptrdiff_t fp_depth = bt_depth(fp_bt);

  printf("Printing frame pointer backtrace, %td frames:\n", fp_depth);
  gSymbolize->BackTrace(fp_bt, 0, 0);

  const unsigned int fp_max = static_cast<unsigned int>(fp_depth - 2);
  constexpr unsigned int fp_bias = 3;
  printf(
      "Printing frame pointer backtrace, %td frames but"
      " starting at #%u and truncated to %u frames total:\n",
      fp_depth, fp_bias, fp_max);
  gSymbolize->BackTrace(fp_bt, fp_bias, fp_bias + fp_max);

  const auto scs_bt = CollectScs();
  const ptrdiff_t scs_depth = bt_depth(scs_bt);
  if (BootShadowCallStack::kEnabled) {
    printf("Printing shadow call stack backtrace, %td frames:\n", scs_depth);
    gSymbolize->BackTrace(scs_bt, 0, 0);

    const unsigned int scs_max = static_cast<unsigned int>(scs_depth - 2);
    constexpr unsigned int scs_bias = 3;
    printf(
        "Printing shadow call stack backtrace, %td frames but"
        " starting at #%u and truncated to %u frames total:\n",
        scs_depth, scs_bias, scs_max);
    gSymbolize->BackTrace(scs_bt, scs_bias, scs_bias + scs_max);

    ZX_ASSERT(fp_depth == scs_depth);

    struct Both {
      decltype(fp_bt.begin()) fp;
      decltype(scs_bt.begin()) scs;
      bool first = true;
    };
    for (auto [fp, scs, first] = Both{fp_bt.begin(), scs_bt.begin()}; fp != fp_bt.end();
         ++fp, ++scs, first = false) {
      ZX_ASSERT(scs != scs_bt.end());

      // The first PC is the collection call site above, which differs between
      // the two collections.  The rest should match.
      if (first) {
        ZX_ASSERT_MSG(*scs != *fp, "SCS %#" PRIxPTR " vs FP %#" PRIxPTR, *scs, *fp);
      } else {
        ZX_ASSERT_MSG(*scs == *fp, "SCS %#" PRIxPTR " vs FP %#" PRIxPTR, *scs, *fp);
      }
    }
  } else {
    ZX_ASSERT(scs_bt.empty());
    ZX_ASSERT(scs_depth == 0);
  }

  return fp_depth - 1;
}

[[gnu::noinline]] PHYS_SINGLETHREAD ptrdiff_t Outer() { return Find() - 1; }

[[gnu::noinline]] PHYS_SINGLETHREAD ptrdiff_t Otter() { return Outer() - 1; }

[[gnu::noinline]] PHYS_SINGLETHREAD ptrdiff_t Foo() { return Otter() - 1; }

extern "C" [[gnu::noinline]] PHYS_SINGLETHREAD ptrdiff_t CalledFromAsmWithPrologue() {
  return Otter() - 1;
}

// To test assembly macros used on various platforms, we need to call a function
// that uses .prologue.fp/.epilogue.fp, and ensure that the macros follow the
// calling convention for each target architecture.
extern "C" ptrdiff_t CallerWithAsmPrologue();

ptrdiff_t PHYS_SINGLETHREAD CheckAsmMacros() {
  ptrdiff_t entry_depth = Foo();
  ptrdiff_t from_asm_depth = CallerWithAsmPrologue();
  ptrdiff_t exit_depth = Foo();
  ZX_ASSERT(exit_depth == entry_depth);
  return from_asm_depth;
}

}  // namespace

[[gnu::noinline]] int TestMain(void* bootloader_data, ktl::optional<EarlyBootZbi> zbi,
                               arch::EarlyTicks) {
  MainSymbolize symbolize("backtrace-test");

  if (zbi) {
    ZX_ASSERT(Foo() == 4);  // _start -> PhysMain -> ZbiMain -> TestMain -> Foo -> Otter...
    ZX_ASSERT(CheckAsmMacros() ==
              5);  // _start -> PhysMain -> ZbiMain -> TestMain -> CallerWithAsmPrologue -> Otter...
  } else {
    ZX_ASSERT(Foo() == 3);             // _start -> PhysMain -> TestMain -> Foo -> Otter...
    ZX_ASSERT(CheckAsmMacros() == 4);  // _start -> PhysMain -> TestMain -> CallerWithAsmPrologue ->
                                       // CalledFromAsmWithPrologue -> Otter...
  }
  return 0;
}
