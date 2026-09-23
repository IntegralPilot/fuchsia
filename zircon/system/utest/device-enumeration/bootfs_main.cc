// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <iostream>

#include <fbl/string.h>
#include <fbl/vector.h>

#include "aemu.h"
#include "common.h"

namespace {

fbl::String GetTestFilter() {
  zx::result sys_info = component::Connect<fuchsia_sysinfo::SysInfo>();
  if (sys_info.is_error()) {
    return "Unknown";
  }

  const fidl::WireResult result = fidl::WireCall(sys_info.value())->GetBoardName();
  if (!result.ok()) {
    return "Unknown";
  }
  const fidl::WireResponse response = result.value();
  if (response.status != ZX_OK) {
    return "Unknown";
  }
  const std::string_view board_name = response.name.get();

  std::cout << "Found board " << board_name << std::endl;

  if (board_name == "qemu-arm64") {
    return "*QemuArm64*";
  } else if (board_name == "qemu-riscv64") {
    return "*QemuRiscv64*";
  } else if (board_name == "vim3") {
    return "*Vim3*";
  } else if (board_name == "astro") {
    return "*Astro*";
  } else if (board_name == "sherlock") {
    return "*Sherlock*";
  } else if (board_name == "nelson") {
    return "*Nelson*";
  } else if (board_name == "NUC7i5DNB" || board_name == "NUC11TNBv5") {
    return "*Nuc*";
  } else if (board_name == "Standard PC (Q35 + ICH9, 2009)") {
    // QEMU and AEMU with emulated Q35 boards have this board name.
    if (device_enumeration::IsAemuBoard()) {
      return "*AemuX64*";
    }
    return "*QemuX64*";
  } else if (board_name == "Google Compute Engine") {
#ifdef __aarch64__
    return "*GceArm64*";
#endif
  } else if (board_name == "arm64" || board_name == "x64") {
    return "*GenericShouldFail*";
  }

  return "Unknown";
}

// If this test fails, it indicates that the board driver set the board name incorrectly.
TEST_F(DeviceEnumerationTest, GenericShouldFailTest) {
  ASSERT_TRUE(false,
              "Board name was a generic board name, likely indicating that the board driver failed "
              "to find a real board name.");
}

}  // namespace

int main(int argc, char** argv) {
  std::vector<std::string> errors;
  auto options = zxtest::Runner::Options::FromArgs(argc, argv, &errors);
  zxtest::LogSink* log_sink = zxtest::Runner::GetInstance()->mutable_reporter()->mutable_log_sink();

  if (!errors.empty()) {
    for (const auto& error : errors) {
      log_sink->Write("%s\n", error.c_str());
    }
    options.help = true;
  }

  options.filter = fbl::StringPrintf("%s:%s", GetTestFilter().c_str(), options.filter.c_str());

  // Errors will always set help to true.
  if (options.help) {
    zxtest::Runner::Options::Usage(argv[0], log_sink);
    return errors.empty();
  }

  if (options.list) {
    zxtest::Runner::GetInstance()->List(options);
    return 0;
  }

  return zxtest::Runner::GetInstance()->Run(options);
}
