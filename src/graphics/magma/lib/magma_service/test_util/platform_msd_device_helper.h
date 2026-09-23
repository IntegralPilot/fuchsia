// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_PLATFORM_MSD_DEVICE_HELPER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_PLATFORM_MSD_DEVICE_HELPER_H_

namespace msd {
struct DeviceHandle;
}

// Gets the handle that can be passed to msd::CreateDevice. This function must be implemented by the
// MSD.
msd::DeviceHandle* GetTestDeviceHandle();

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_PLATFORM_MSD_DEVICE_HELPER_H_
