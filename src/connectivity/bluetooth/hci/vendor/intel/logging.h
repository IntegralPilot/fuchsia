// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_LOGGING_H_
#define SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_LOGGING_H_

#include <lib/driver/logging/cpp/logger.h>

#define errorf(fmt, args...) fdf::error(fmt, ##args)
#define warnf(fmt, args...) fdf::warn(fmt, ##args)
#define infof(fmt, args...) fdf::info(fmt, ##args)
#define tracef(fmt, args...) fdf::trace(fmt, ##args)

#endif  // SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_LOGGING_H_
