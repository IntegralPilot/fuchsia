// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_METADATA_H_
#define SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_METADATA_H_

#include <lib/ddk/metadata.h>

#define NTC_CHANNELS_METADATA_PRIVATE (0x4e544300 | DEVICE_METADATA_PRIVATE)  // NTCd
#define NTC_PROFILE_METADATA_PRIVATE (0x4e545000 | DEVICE_METADATA_PRIVATE)   // NTPd

#endif  // SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_METADATA_H_
