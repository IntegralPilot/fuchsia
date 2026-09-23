// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_BIN_CLOCK_CLOCK_DEBUG_H_
#define SRC_DEVICES_CLOCK_BIN_CLOCK_CLOCK_DEBUG_H_

#include <fidl/fuchsia.hardware.clock/cpp/fidl.h>

namespace clock_debug {

struct Clock {
  uint32_t id;
  std::string name;
  fidl::ClientEnd<fuchsia_hardware_clock::Clock> clock_client;
};

std::vector<Clock> ListClocks();

Clock GetClock(uint32_t id);

void PrintClocks(const std::vector<Clock>& clocks, bool verbose);

void ShowClock(const Clock& clock);

void QueryRate(const Clock& clock, uint64_t rate);

void EnableClock(const Clock& clock);

void DiableClock(const Clock& clock);

void ClockSetRate(const Clock& clock, uint64_t rate);

void ClockSetInput(const Clock& clock, uint32_t index);

}  // namespace clock_debug

#endif  // SRC_DEVICES_CLOCK_BIN_CLOCK_CLOCK_DEBUG_H_
