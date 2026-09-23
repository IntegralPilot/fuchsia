// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

#include <lib/driver/component/cpp/driver_base.h>

namespace fdf {
bool logger_wait_for_initial_interest = false;
}  // namespace fdf

#endif
