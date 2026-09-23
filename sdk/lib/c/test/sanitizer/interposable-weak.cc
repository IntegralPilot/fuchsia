// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a DSO that does not (explicitly) depend on any external symbols. It has a
// constructor that depends on a weak symbol it defines, but may be interposed
// by a stronger definition in another library.

#include <zircon/compiler.h>

#include "ctor-order-test.h"

Global global;

__EXPORT __WEAK InterposeStatus gInterposable = SuccessfullyInterposedByWeakSymbol;

Global::Global() {
  // During construction, this library accesses one of its own globals which may be
  // interposed by another library.
  interposed = gInterposable;
}
