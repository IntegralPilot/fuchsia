// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/channel.h>
#include <stdio.h>

#include <filesystem>

#include "args.h"
#include "i2cutil2.h"

// LINT.IfChange
constexpr char kUsageSummary[] = R"""(
Usage:
  i2cutil read <device> <address> [<address>...]
  i2cutil write <device> <address> [<address>...] <data> [<data>...]
  i2cutil transact <device> (r <bytes>|w <address> [<address>...] [<data>...])...
  i2cutil dump <device> <address> <count>
  i2cutil list
  i2cutil ping
  i2cutil help
)""";

constexpr char kUsageDetails[] = R"""(
Commands:
  read | r              Read one byte from an I2C device. Use `i2cutil transact`
                        to read multiple bytes. <device> can be the full path of
                        a devfs node (example: `/dev/class/i2c/031`) or only the
                        devfs node's index (example: `31`) or it can simply be the
                        device's friendly named obtained via `i2cutil list`
                        (example: `pmic`). Use `i2cutil ping` to get devfs node
                        paths and indexes. <address> is the internal register of
                        <device> to read from. Use multiple <address> values to
                        access a multi-byte (little-endian) register address.
                        For example `i2cutil read 4 0x20 0x3D` to read the
                        register at `0x203D`.
  write | w             Write one or more bytes (<data>) to an I2C device. See the
                        `i2cutil read` description for explanations of <device>
                        and <address>.
  transact | t          Perform a transaction with multiple segments. Each segment
                        can be a write (`w`) or a read (`r`).
  dump | d              Reads and prints <count> registers from <device> starting
                        at the address indicated by <address>.
  list | l              List all the I2C devices available on the system.
  ping | p              Ping all I2C devices under devfs path `/dev/class/i2c` by
                        reading from each device's 0x00 address.
  help | h              Print this help text.

Examples:
  Read one byte from the register at `0x20` of the I2C device represented by
  devfs node index `4`:
  $ i2cutil read 4 0x20

  Read three bytes from the same I2C device and register as the last example:
  $ i2cutil transact 4 w 0x20 r 3

  Read one byte from the register at the multi-byte address `0x203D` of the
  I2C device named "temp_sensor":
  $ i2cutil read temp_sensor 0x20 0x3D

  Same as the last example but represent the I2C device with a devfs node path:
  $ i2cutil read /dev/class/i2c/004 0x20 0x3D

  Write byte `0x12` to the register at `0x2C` of the I2C device represented by
  devfs node index `3`:
  $ i2cutil write 3 0x2C 0x12

  Write byte `0x121B` to the same device and register as the last example:
  $ i2cutil write 3 0x2C 0x12 0x1B

  Write byte `0x1B to register `0x2C12` of a different device (note that
  this is the exact same command as the last example; the meaning of the
  arguments depends on the I2C device):
  $ i2cutil write 3 0x2C 0x12 0x1B

  Ping all I2C devices:
  $ i2cutil ping
  /dev/class/i2c/821: OK
  /dev/class/i2c/822: OK
  /dev/class/i2c/823: OK
  /dev/class/i2c/824: OK
  Error ZX_ERR_TIMED_OUT
  /dev/class/i2c/825: ERROR

  List all I2C devices:
  $ i2cutil list
  159: pmic
  160: (ANONYMOUS)
  161: temp_sensor

  Print 3 registers from the device named `pmic` starting at address 0x10:
  $ i2cutil dump pmic 0x10 3
  0x10: 0x41
  0x11: 0x00
  0x12: 0x00

Notes:
  Source code for `i2cutil`: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/i2c/bin/i2cutil.cc
)""";
// LINT.ThenChange(//docs/reference/tools/hardware/i2cutil.md)

namespace {

constexpr char kI2cDevicePath[] = "/dev/class/i2c";

void usage(bool show_details) {
  printf(kUsageSummary);
  if (!show_details) {
    printf("\nUse `i2cutil help` to see full help text\n");
  } else {
    printf(kUsageDetails);
  }
}

void printBytes(const std::vector<uint8_t>& bytes) {
  for (const uint8_t b : bytes) {
    printf("0x%02x ", b);
  }
}

void printTransactions(const std::vector<i2cutil::TransactionData>& transactions) {
  for (const auto& transaction : transactions) {
    if (transaction.type == i2cutil::TransactionType::Read) {
      printf("Read:  ");
    } else if (transaction.type == i2cutil::TransactionType::Write) {
      printf("Write: ");
    }
    printBytes(transaction.bytes);
    printf("\n");
  }
}

void printDump(const std::vector<i2cutil::TransactionData>& transactions) {
  // An I2C dump will be a sequence of alternating writes and reads.
  // {W R  W R  W R  W R}: Each write represents the address of the register
  // being read and the following read will represent the data read back from
  // that register.
  // Start by ensuring that the input conforms to this spec.
  ZX_ASSERT(transactions.size() % 2 == 0);
  for (size_t i = 0; i < transactions.size(); i++) {
    if (i % 2 == 0) {
      ZX_ASSERT(transactions[i].type == i2cutil::TransactionType::Write);
      printf("0x%02x: ", transactions[i].bytes.front());
    } else {
      ZX_ASSERT(transactions[i].type == i2cutil::TransactionType::Read);
      printf("0x%02x\n", transactions[i].bytes.front());
    }
  }
}

zx_status_t ping() {
  for (auto& dev_path : std::filesystem::directory_iterator(kI2cDevicePath)) {
    i2cutil::TransactionData wr;
    wr.type = i2cutil::TransactionType::Write;
    wr.bytes.push_back(0x00);
    i2cutil::TransactionData rd;
    rd.type = i2cutil::TransactionType::Read;
    rd.count = 1;

    std::vector<i2cutil::TransactionData> txns = {wr, rd};

    zx::result device = component::Connect<fuchsia_hardware_i2c::Device>(dev_path.path().c_str());
    if (device.is_error()) {
      printf("%s: ERROR (%s)\n", dev_path.path().c_str(), device.status_string());
      continue;
    }

    zx_status_t result = execute(std::move(device.value()), txns);
    if (result != ZX_OK) {
      printf("%s: ERROR (%s)\n", dev_path.path().c_str(), zx_status_get_string(result));
      continue;
    }

    printf("%s: OK\n", dev_path.path().c_str());
  }

  return ZX_OK;
}

zx_status_t list() {
  for (auto& dev_path : std::filesystem::directory_iterator(kI2cDevicePath)) {
    zx::result device = component::Connect<fuchsia_hardware_i2c::Device>(dev_path.path().c_str());
    if (device.is_error()) {
      printf("%s: ERROR (%s)\n", dev_path.path().filename().c_str(), device.status_string());
      continue;
    }

    // Try to get the name of the device.
    const fidl::WireResult result = fidl::WireCall(device.value())->GetName();
    if (!result.ok()) {
      printf("%s: ERROR (%s)\n", dev_path.path().filename().c_str(), result.status_string());
      continue;
    }

    const fit::result response = result.value();
    if (response.is_error()) {
      if (response.error_value() == ZX_ERR_NOT_SUPPORTED) {
        // ZX_ERR_NOT_SUPPORTED is part of the i2c.fidl contract and means that
        // the device did not return a friendly name.
        printf("%s: (ANONYMOUS)\n", dev_path.path().filename().c_str());
      } else {
        printf("%s: ERROR (%s)\n", dev_path.path().filename().c_str(),
               zx_status_get_string(response.error_value()));
      }
      continue;
    }

    const std::string name(response.value()->name.get());
    printf("%s: %s\n", dev_path.path().filename().c_str(), name.c_str());
  }

  return ZX_OK;
}

zx::result<fidl::ClientEnd<fuchsia_hardware_i2c::Device>> get_device_by_name(
    const std::string& target) {
  for (auto& dev_path : std::filesystem::directory_iterator(kI2cDevicePath)) {
    zx::result device = component::Connect<fuchsia_hardware_i2c::Device>(dev_path.path().c_str());
    if (device.is_error()) {
      continue;
    }

    const fidl::WireResult result = fidl::WireCall(device.value())->GetName();
    if (!result.ok()) {
      continue;
    }

    const fit::result response = result.value();
    if (response.is_error()) {
      continue;
    }

    const std::string name(response.value()->name.get());
    if (name == target) {
      return zx::ok(std::move(device.value()));
    }
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

}  // namespace

int main(int argc, const char* argv[]) {
  auto args = i2cutil::Args::FromArgv(argc, argv);

  if (args.is_error()) {
    usage(false);
    return -1;
  }

  if (args->Op() == i2cutil::I2cOp::Read || args->Op() == i2cutil::I2cOp::Write ||
      args->Op() == i2cutil::I2cOp::Transact || args->Op() == i2cutil::I2cOp::Dump) {
    zx::result<fidl::ClientEnd<fuchsia_hardware_i2c::Device>> device;
    if (std::filesystem::exists(args->Path())) {
      device = component::Connect<fuchsia_hardware_i2c::Device>(args->Path());
    } else {
      device = get_device_by_name(args->Path());
    }

    if (device.is_error()) {
      fprintf(stderr, "i2cutil: Failed to connect to device: %s\n", device.status_string());
      return -1;
    }

    zx_status_t result = execute(std::move(device.value()), args->Transactions());
    if (result != ZX_OK) {
      fprintf(stderr, "i2cutil: Failed to execute transactions, st = %s\n",
              zx_status_get_string(result));
      return -1;
    }
    if (args->Op() == i2cutil::I2cOp::Dump) {
      printDump(args->Transactions());
    } else {
      printTransactions(args->Transactions());
    }
  } else if (args->Op() == i2cutil::I2cOp::Ping) {
    return ping() == ZX_OK ? 0 : -1;
  } else if (args->Op() == i2cutil::I2cOp::List) {
    return list() == ZX_OK ? 0 : -1;
  } else if (args->Op() == i2cutil::I2cOp::Help) {
    usage(true);
  } else {
    fprintf(stderr, "i2cutil: Unknown command\n");
    return -1;
  }

  return 0;
}
