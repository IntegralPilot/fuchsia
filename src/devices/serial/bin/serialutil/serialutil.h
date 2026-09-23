// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_SERIAL_BIN_SERIALUTIL_SERIALUTIL_H_
#define SRC_DEVICES_SERIAL_BIN_SERIALUTIL_SERIALUTIL_H_

#include <fidl/fuchsia.hardware.serial/cpp/fidl.h>

namespace serial {
class SerialUtil {
 public:
  // Executes the `serialutil` program.
  //
  // @param argc The command-line argument count.
  // @param argv The command-line arguments.
  // @param connection An optional connection to use for testing. Any specified device paths will
  // be ignored if this is set.
  static int Execute(int argc, char* argv[],
                     fidl::ClientEnd<fuchsia_hardware_serial::DeviceProxy> connection);
};
}  // namespace serial

#endif  // SRC_DEVICES_SERIAL_BIN_SERIALUTIL_SERIALUTIL_H_
