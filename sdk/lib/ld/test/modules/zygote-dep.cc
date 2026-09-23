// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include "zygote.h"

__EXPORT int zygote_dep() { return called_by_zygote_dep() + *initialized_data[1]++; }
