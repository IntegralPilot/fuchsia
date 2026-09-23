// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <zircon/assert.h>

#include <dev/init.h>
#include <phys/handoff.h>

void DriverHandoffEarly(const PhysHandoff& handoff) {
  UartDriverHandoffEarly(handoff.uart);
  PlatformDriverHandoffEarly(handoff.arch_handoff);
}

void DriverHandoffPostVm(const PhysHandoff& handoff) {
  UartDriverHandoffPostVm(handoff.uart);
  PlatformDriverHandoffPostVm(handoff.arch_handoff);
}

void DriverHandoffLate(const PhysHandoff& handoff) {
  UartDriverHandoffLate(handoff.uart);
  PlatformDriverHandoffLate(handoff.arch_handoff);
}
