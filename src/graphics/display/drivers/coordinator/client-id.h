// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_ID_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_ID_H_

#include <cstdint>

#include <fbl/strong_int.h>

namespace display_coordinator {

// Typesafe unique identifier for an active Coordinator client connection.
//
// Each value is uniquely associated with one Coordinator client connection,
// throughout the Coordinator's entire lifetime. In other words, the Coordinator
// does not reuse ClientId values.
DEFINE_STRONG_INT(ClientId, uint64_t);

// Sentinel value that will never be associated with a client connection.
constexpr ClientId kInvalidClientId(0);

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_ID_H_
