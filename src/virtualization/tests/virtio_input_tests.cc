// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/ui/test/input/cpp/fidl.h>
#include <lib/zx/clock.h>

#include <string>

#include <gtest/gtest.h>

#include "src/virtualization/tests/lib/enclosed_guest.h"
#include "src/virtualization/tests/lib/guest_test.h"

namespace {

// Create an alias, as "TEST_F" requires the fixture name to be a valid C token.
using VirtioInputDebianGuestTest = GuestTest<DebianGpuEnclosedGuest>;

TEST_F(VirtioInputDebianGuestTest, Input) {
  // We need to wait for a display because we route input from that display to the input device.
  this->GetEnclosedGuest().WaitForDisplay();

  // Start the test.
  auto& guest_console = this->GetEnclosedGuest().GetConsole();
  EXPECT_EQ(
      guest_console->SendBlocking("/test_utils/virtio_input_test_util keyboard /dev/input/event*\n",
                                  zx::time::infinite()),
      ZX_OK);

  // Wait for the test utility to print its string ("type ..."), and then
  // send keystrokes.
  EXPECT_EQ(guest_console->WaitForMarker("Type 'abc<shift>'", zx::time::infinite()), ZX_OK);

  // Inject the string 'abcD'. This last D will result in the series of key-presses "shift-down,
  // d-down, d-up, shift-up", which gets us the shift press we need.
  auto input_registry =
      this->GetEnclosedGuest().template ConnectToService<fuchsia::ui::test::input::Registry>();
  fuchsia::ui::test::input::KeyboardPtr keyboard;
  bool keyboard_registered = false;
  fuchsia::ui::test::input::RegistryRegisterKeyboardRequest request;
  request.set_device(keyboard.NewRequest());
  input_registry->RegisterKeyboard(std::move(request), [&]() { keyboard_registered = true; });
  this->RunLoopUntil([&] { return keyboard_registered; }, zx::time::infinite());
  bool input_injected = false;

  fuchsia::ui::test::input::KeyboardSimulateUsAsciiTextEntryRequest text_request;
  text_request.set_text("abcD");
  keyboard->SimulateUsAsciiTextEntry(std::move(text_request),
                                     [&input_injected]() { input_injected = true; });
  RunLoopUntil([&] { return input_injected; }, zx::time::infinite());

  // Ensure we passed.
  std::string result;
  EXPECT_EQ(guest_console->WaitForMarker("PASS", zx::time::infinite(), &result), ZX_OK);
}

}  // namespace
