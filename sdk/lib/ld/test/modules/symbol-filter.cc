// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "symbol-filter.h"

#include "test-start.h"

int64_t TestStart() { return first() + second() + third(); }
