// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a DSO that depends explicitly on `global` defined in
// libinterposable.so. That global has a constructor which references an
// interposable symbol for which this DSO should provide the strong definition
// to.

#include <zircon/compiler.h>

#include "ctor-order-test.h"

// This interposes the same symbol defined in libinterposable.so.
__EXPORT InterposeStatus gInterposed = DidNotInterpose;

extern "C" __EXPORT int gInterposing = 0;
