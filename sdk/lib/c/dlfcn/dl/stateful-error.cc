// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "stateful-error.h"

#include <zircon/compiler.h>

namespace dl {

// This goes into .tbss and has no constructor call needed, but it has a
// destructor that must run if it's ever been used so no buffers are leaked.
__CONSTINIT thread_local StatefulError gPerThreadErrorState;

}  // namespace dl
