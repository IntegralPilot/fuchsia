// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/test_data/arm_ehabi_unwind_table.h"

namespace {
int ReturnOne() { return 1; }

int ReturnNPlusOne(int n) { return n + ReturnOne(); }
}  // namespace

class MyClass {
 public:
  struct Inner {
    NOINLINE static int MyMemberTwo();
  };

  NOINLINE int MyMemberOne() { return 42; }
};

NOINLINE int MyClass::Inner::MyMemberTwo() { return 61; }

EXPORT int MyFunction(int i) {
  MyClass my_class;
  return my_class.MyMemberOne() + ReturnOne() + MyClass::Inner::MyMemberTwo() + ReturnNPlusOne(i);
}
