// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a DSO that does not (explicitly) depend on any external symbols. It has a
// constructor that only depends on a symbol it defines.

#include <zircon/compiler.h>

__EXPORT int x;

struct Global {
  Global() { member = x; }
  int member;
};

Global global;

// This is a uniquely defined global we can check via dlsym just to make sure
// this DSO loaded correctly.
extern "C" __EXPORT int gNoDeps = 0;
