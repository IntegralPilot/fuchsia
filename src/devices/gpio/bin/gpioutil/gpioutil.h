// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_GPIO_BIN_GPIOUTIL_GPIOUTIL_H_
#define SRC_DEVICES_GPIO_BIN_GPIOUTIL_GPIOUTIL_H_

#include <fidl/fuchsia.hardware.pin/cpp/wire.h>
#include <lib/zx/result.h>
#include <stdio.h>

enum GpioFunc {
  Read,
  SetBufferMode,
  Interrupt,
  Configure,
  GetName,
  List,
  GetDriveStrength,
  Invalid
};

template <typename T, typename ReturnType>
zx::result<ReturnType> GetStatus(const T& result);

template <typename T>
zx::result<> GetStatus(const T& result);

// Parse the command line arguments in |argv|
int ParseArgs(int argc, char** argv, GpioFunc* func, fidl::AnyArena& arena,
              fuchsia_hardware_gpio::BufferMode* buffer_mode,
              fuchsia_hardware_gpio::InterruptMode* interrupt_mode,
              fuchsia_hardware_pin::wire::Configuration* config);

zx::result<> ListGpios();

zx::result<fidl::WireSyncClient<fuchsia_hardware_pin::Debug>> FindDebugClientByName(
    std::string_view name);

int ClientCall(fidl::WireSyncClient<fuchsia_hardware_pin::Debug> client, GpioFunc func,
               fidl::AnyArena& arena, fuchsia_hardware_gpio::BufferMode buffer_mode,
               fuchsia_hardware_gpio::InterruptMode interrupt_mode,
               fuchsia_hardware_pin::wire::Configuration config);

#endif  // SRC_DEVICES_GPIO_BIN_GPIOUTIL_GPIOUTIL_H_
