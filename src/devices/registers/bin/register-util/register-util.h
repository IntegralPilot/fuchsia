// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_REGISTERS_BIN_REGISTER_UTIL_REGISTER_UTIL_H_
#define SRC_DEVICES_REGISTERS_BIN_REGISTER_UTIL_REGISTER_UTIL_H_

#include <fidl/fuchsia.hardware.registers/cpp/wire.h>

int run(int argc, const char** argv, fidl::ClientEnd<fuchsia_hardware_registers::Device> channel);

#endif  // SRC_DEVICES_REGISTERS_BIN_REGISTER_UTIL_REGISTER_UTIL_H_
