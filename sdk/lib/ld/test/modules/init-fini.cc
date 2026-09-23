// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <stdint.h>

#include "test-start.h"

namespace {

int a;

void test(int n, int o) {
  int* ap = &a;
  __asm__("" : "=r"(ap) : "0"(ap));
  if (std::exchange(*ap, n) != o) {
    __builtin_trap();
  }
}

// TODO(https://fxbug.dev/42083949): clang doesn't support template arguments being used
// as part of attribute arguments.

#ifdef __clang__

[[gnu::constructor(101)]] void ctor0() { test(1, 0); }
[[gnu::constructor(102)]] void ctor1() { test(2, 1); }

[[gnu::destructor(102)]] void dtor1() { test(3, 2); }
[[gnu::destructor(101)]] void dtor2() { test(4, 3); }

#else

template <int New, int Old>
[[gnu::constructor(100 + New)]] void ctor() {
  test(New, Old);
}

template <int New, int Old>
[[gnu::destructor(105 - New)]] void dtor() {
  test(New, Old);
}

template void ctor<1, 0>();
template void ctor<2, 1>();

template void dtor<3, 2>();
template void dtor<4, 3>();

#endif

}  // namespace

extern "C" int64_t TestStart() {
  const auto& executable_module = *ld::abi::_ld_abi.loaded_modules.get();

  if (executable_module.init.size() != 2) {
    return 1;
  }
  executable_module.init.CallInit(executable_module.link_map.addr);
  if (a != 2) {
    return 2;
  }

  if (executable_module.fini.size() != 2) {
    return 3;
  }
  executable_module.fini.CallFini(executable_module.link_map.addr);
  if (a != 4) {
    return 4;
  }

  return 17;
}
