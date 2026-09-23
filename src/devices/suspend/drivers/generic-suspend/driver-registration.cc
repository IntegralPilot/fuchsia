// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export2.h>

#include "generic-suspend.h"

// Register the production version of the GenericSuspend driver.
FUCHSIA_DRIVER_EXPORT2(suspend::GenericSuspend);
