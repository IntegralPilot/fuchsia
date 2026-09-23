// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include "symbol-filter.h"

__EXPORT int first() { return VALUE * 1; }

__EXPORT int second() { return VALUE * 2; }

__EXPORT int third() { return VALUE * 3; }
