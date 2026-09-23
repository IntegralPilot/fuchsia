// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/iwlwifi/platform/time.h"

#include <lib/async/time.h>

zx_time_t iwl_time_now(struct device* dev) { return async_now(dev->task_dispatcher); }
