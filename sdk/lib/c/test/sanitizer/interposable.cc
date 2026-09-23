// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a DSO that does not (explicitly) depend on any external symbols. It
// has a module ctor that is interposable but not interposed.

#include <zircon/compiler.h>

#include "ctor-order-test.h"

Global global;

__EXPORT InterposeStatus gInterposable = SuccessfullyInterposed;

Global::Global() { interposed = gInterposable; }

// This is just a public-facing synbol we can use to check global.interposed.
extern "C" __EXPORT InterposeStatus *gInterposedPtr = &global.interposed;
