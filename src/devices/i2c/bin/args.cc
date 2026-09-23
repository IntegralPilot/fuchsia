// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "args.h"

#include <zircon/assert.h>

#include <iostream>
#include <limits>
#include <map>
#include <optional>
#include <sstream>

namespace {

namespace PositionalArgs {
constexpr size_t kCommand = 1;  // First argument is the subcommand
constexpr size_t kPath = 2;     // Second argument is the device name or path
constexpr size_t kVargs = 3;    // Variable arguments (data / parameters follow)

}  // namespace PositionalArgs

const std::map<char, i2cutil::I2cOp> kOrdinalToOperation = {
    {'r', i2cutil::I2cOp::Read}, {'w', i2cutil::I2cOp::Write}, {'t', i2cutil::I2cOp::Transact},
    {'d', i2cutil::I2cOp::Dump}, {'l', i2cutil::I2cOp::List},  {'p', i2cutil::I2cOp::Ping},
    {'h', i2cutil::I2cOp::Help},
};

const std::map<i2cutil::I2cOp, int> kOperationToMinArgCount = {
    {i2cutil::I2cOp::Read, 4}, {i2cutil::I2cOp::Write, 4}, {i2cutil::I2cOp::Transact, 5},
    {i2cutil::I2cOp::Dump, 5}, {i2cutil::I2cOp::List, 2},  {i2cutil::I2cOp::Ping, 2},
    {i2cutil::I2cOp::Help, 2},
};

std::string inferPath(const char* arg) {
  /// i2cutil accepts arguments in the form of full paths or simply the device
  /// number in /dev/class/i2c/...
  /// This function guesses which format is being provided and infers the path
  /// accordingly. If a number N is passed the path is inferred as
  /// /dev/class/i2c/NNN otherwise `arg` is returned unchanged.
  /// Note that the path returned by this function is not guaranteed to be a
  /// valid i2c device.
  std::string result = arg;

  char buffer[32];
  int id = -1;
  if (sscanf(arg, "%u", &id) == 1) {
    if (snprintf(buffer, std::size(buffer), i2cutil::kI2cPathFormat, id) >= 0) {
      result = buffer;
    }
  }

  return result;
}

constexpr int64_t kBadParse = -1;
int64_t parse_positive_long(const char* number) {
  char* end;
  uint64_t result = strtoul(number, &end, 0);
  if (end == number || *end != '\0' || result > std::numeric_limits<int64_t>::max()) {
    return kBadParse;
  }
  return static_cast<int64_t>(result);
}

zx_status_t readArgs(const char* argv[], size_t count, std::vector<uint8_t>& data) {
  for (size_t i = 0; i < count; i++) {
    int64_t result = parse_positive_long(argv[i]);
    if (result < 0 || result > 0xff) {
      return ZX_ERR_INVALID_ARGS;
    }

    uint8_t byte = static_cast<uint8_t>(result);
    data.push_back(byte);
  }

  return ZX_OK;
}

zx_status_t parseTransactionData(const char* argv[], size_t count,
                                 std::vector<i2cutil::TransactionData>& transactions) {
  enum class State { Start, ScanningWrite, ScanningRead };

  State s = State::Start;
  i2cutil::TransactionData current;

  for (size_t i = 0; i < count; i++) {
    switch (s) {
      case State::Start:
        // Only 'r' and 'w' are valid.
        if (argv[i][0] == 'r') {
          s = State::ScanningRead;
          current.Reset();
          current.type = i2cutil::TransactionType::Read;
        } else if (argv[i][0] == 'w') {
          s = State::ScanningWrite;
          current.Reset();
          current.type = i2cutil::TransactionType::Write;
        } else {
          // Parse error, stream must start with either 'w' or 'r'
          return ZX_ERR_INVALID_ARGS;
        }
        break;
      case State::ScanningRead: {
        // Scan one integer that represents the number of bytes to read then
        // return to the State::Start state.
        int64_t result = parse_positive_long(argv[i]);
        if (result < 0 || result > 0xff)
          return ZX_ERR_INVALID_ARGS;
        current.count = result;
        transactions.push_back(current);
        s = State::Start;
        break;
      }
      case State::ScanningWrite:
        // Scan bytes to write until a new transaction is started with either
        // 'r' or 'w'
        if (argv[i][0] == 'r') {
          transactions.push_back(current);
          current.Reset();
          current.type = i2cutil::TransactionType::Read;
          s = State::ScanningRead;
        } else if (argv[i][0] == 'w') {
          transactions.push_back(current);
          current.Reset();
          current.type = i2cutil::TransactionType::Write;
          s = State::ScanningWrite;
        } else {
          int64_t result = parse_positive_long(argv[i]);
          if (result < 0 || result > 0xff)
            return ZX_ERR_INVALID_ARGS;
          current.bytes.push_back(static_cast<uint8_t>(result));
        }
        break;
    }
  }

  // TODO(gkalsi): Filter out empty transactions? Make sure there's at least
  // one transaction available to run?

  // Push the last transaction onto the result list as well.
  if (s == State::ScanningWrite) {
    transactions.push_back(current);
  }

  return ZX_OK;
}

}  // namespace

namespace i2cutil {

zx::result<Args> Args::FromArgv(const int argc, const char* argv[]) {
  if (argc < 2) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  Args result;

  // Step 1: Parse which command the user is trying to invoke.
  auto op_it = kOrdinalToOperation.find(argv[PositionalArgs::kCommand][0]);
  if (op_it != kOrdinalToOperation.end()) {
    result.op_ = op_it->second;
  } else {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Step 2: Figure out how many args this command expects, and ensure that the
  //         user has supplied enough args.
  auto minArgCountIt = kOperationToMinArgCount.find(result.op_);
  ZX_ASSERT(minArgCountIt != kOperationToMinArgCount.end());
  if (argc < minArgCountIt->second) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Step 3: If the command expects a path, parse the path; otherwise we're done.
  if (minArgCountIt->second >= 3) {
    result.path_ = inferPath(argv[PositionalArgs::kPath]);
  } else {
    return zx::ok(result);
  }

  // Step 4: If the command expects additional data, parse the additional data.
  switch (result.op_) {
    case I2cOp::Read:
    case I2cOp::Write: {
      TransactionData write_data;
      zx_status_t status;
      // Parse a single row of bytes
      write_data.type = TransactionType::Write;
      status =
          readArgs(argv + PositionalArgs::kVargs, argc - PositionalArgs::kVargs, write_data.bytes);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      result.transactions_.push_back(write_data);

      // A Read is just a write followed by 1 byte read.
      if (result.op_ == I2cOp::Read) {
        TransactionData read_data;
        read_data.count = 1;
        read_data.type = TransactionType::Read;
        result.transactions_.push_back(read_data);
      }
      break;
    }
    case I2cOp::Transact: {
      // Parse a sequence of transactions.
      zx_status_t status = parseTransactionData(
          argv + PositionalArgs::kVargs, argc - PositionalArgs::kVargs, result.transactions_);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      break;
    }
    case I2cOp::Dump: {
      int64_t start_address = parse_positive_long(argv[3]);
      int64_t count = parse_positive_long(argv[4]);

      // Only do up to 8-bit addressing for now.
      if (start_address == kBadParse || count == kBadParse || count > 255 || start_address > 255 ||
          (start_address + count) > 255) {
        return zx::error(ZX_ERR_NOT_SUPPORTED);
      }

      for (int64_t i = 0; i < count; i++) {
        TransactionData write;
        TransactionData read;

        write.type = TransactionType::Write;
        write.bytes.push_back(static_cast<uint8_t>(i + start_address));
        result.transactions_.push_back(write);

        read.type = TransactionType::Read;
        read.count = 1;
        result.transactions_.push_back(read);
      }

      break;
    }
    default:
      // No additional data needed, should this be an error?
      break;
  }

  return zx::ok(result);
}

}  // namespace i2cutil
