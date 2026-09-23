// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <lib/fit/defer.h>

#include <gtest/gtest.h>

#include "ctor-order-test.h"

// Each of these tests are no-op tests that just dlopen other libraries and
// invoke module constructors in different orders (depending on the test). For
// hwasan, we also want to ensure false-positive errors are not raised from
// each of these.

namespace {

TEST(HwasanCtorOrderTest, TestNoDeps) {
  // Test that we can dynamically load a single DSO that doesn't reference any
  // external symbols. This is the base case where there are no interposed
  // symbols accessed by other loaded libs.
  void* dl = dlopen("libctor-order-test-no-deps-dso.so", RTLD_NOW);
  auto cleanup = fit::defer([dl]() { dlclose(dl); });
  ASSERT_TRUE(dl) << dlerror();
  void* unique_sym = dlsym(dl, "gNoDeps");
  EXPECT_TRUE(unique_sym) << dlerror();
}

TEST(HwasanCtorOrderTest, TestNoInterposedSymbol) {
  // libctor-order-test-no-interposing-dso.so is a DSO that depends on
  // libctor-order-test-interposable-dso.so, which contains a symbol that is not interposed. The
  // scenario this covers is:
  //
  // (1) library A depends on library B
  // (2) B's constructors are called before A's
  // (3) During construction, B accesses its own globals
  //
  void* dl = dlopen("libctor-order-test-no-interposing-dso.so", RTLD_NOW);
  auto cleanup = fit::defer([dl]() { dlclose(dl); });
  ASSERT_TRUE(dl) << dlerror();
  void* unique_sym = dlsym(dl, "gNoInterposing");
  EXPECT_TRUE(unique_sym) << dlerror();
}

TEST(HwasanCtorOrderTest, TestInterposingSymbol) {
  // libctor-order-test-interposing-dso.so is a DSO that depends on
  // libctor-order-test-interposable-dso.so, which contains a weak symbol that will be interposed.
  // The scenario this covers is:
  //
  // (1) library A (libctor-order-test-interposing-dso.so) depends on library B
  // (libctor-order-test-interposable-dso.so) but also interposes one of B's symbols (2) B's
  // constructors are called before A's (3) During construction, B accesses one of its own globals
  // which was
  //     actually interposed by a symbol in A. This is a unique case which
  //     hwasan needs to consider because it needs to properly register the tag
  //     for the correct interposing global before the access. If it didn't,
  //     hwasan would report a bad load due to a false-positive tag mismatch
  //     for this interposed global. Hwasan should never report this error since
  //     globals should be registered before module ctors are called via
  //     __sanitizer_module_loaded.
  //
  void* dl = dlopen("libctor-order-test-interposing-dso.so", RTLD_NOW);
  auto cleanup = fit::defer([dl]() { dlclose(dl); });
  ASSERT_TRUE(dl) << dlerror();
  void* unique_sym = dlsym(dl, "gInterposing");
  EXPECT_TRUE(unique_sym) << dlerror();
  void* interposed = dlsym(dl, "gInterposedPtr");
  EXPECT_TRUE(interposed) << dlerror();
  EXPECT_EQ(**reinterpret_cast<InterposeStatus**>(interposed), SuccessfullyInterposed);
}

}  // namespace
