// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include <mutex>

#include <zxtest/zxtest.h>

#include "weak.h"

namespace LIBC_NAMESPACE_DECL {

[[gnu::weak]] void NotDefined();
[[gnu::weak]] void NotDefinedWithArgs(int, const char*, double);

namespace {

class LibcTests : public zxtest::Test {
 public:
  static inline bool gCalled = false;
  static inline bool gFakeLock = false;

  void SetUp() override { gCalled = gFakeLock = false; }
  void TearDown() override { gCalled = gFakeLock = false; }

  template <auto Symbol, bool ExpectCall, typename... Args>
  static void Call(Args&&... args) {
    ASSERT_FALSE(gCalled);
    Weak<Symbol>::Call(std::forward<Args>(args)...);
    EXPECT_EQ(gCalled, ExpectCall);
    gCalled = false;
  }
};

void Defined() { LibcTests::gCalled = true; }

void DefinedWithArgs(int i, const char* s, double d) {
  LibcTests::gCalled = true;
  EXPECT_EQ(i, 1);
  EXPECT_STREQ(s, "foo");
  EXPECT_EQ(d, 3.14);
}

void FakeLock() {
  ASSERT_FALSE(LibcTests::gFakeLock);
  LibcTests::gFakeLock = true;
}

void FakeUnlock() {
  ASSERT_TRUE(LibcTests::gFakeLock);
  LibcTests::gFakeLock = false;
}

constexpr WeakLock<FakeLock, FakeUnlock> kFakeWeakLock;
int gLockedThing __TA_GUARDED(kFakeWeakLock);

TEST_F(LibcTests, WeakUndefined) { Call<NotDefined, false>(); }

TEST_F(LibcTests, WeakUndefinedWithArgs) { Call<NotDefinedWithArgs, false>(1, "foo", 3.14); }

TEST_F(LibcTests, WeakDefined) { Call<Defined, true>(); }

TEST_F(LibcTests, WeakDefinedWithArgs) { Call<DefinedWithArgs, true>(1, "foo", 3.14); }

TEST_F(LibcTests, WeakLock) {
  ASSERT_FALSE(LibcTests::gFakeLock);
  {
    std::lock_guard lock(kFakeWeakLock);
    ASSERT_TRUE(LibcTests::gFakeLock);

    // Outside the guard's scope, this would be diagnosed by -Wthread-safety.
    gLockedThing = 0;
  }
  ASSERT_FALSE(LibcTests::gFakeLock);
}

// TODO: WeakOr tests

}  // namespace
}  // namespace LIBC_NAMESPACE_DECL
