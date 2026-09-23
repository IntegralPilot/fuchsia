// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_BIN_ARGS_H_
#define SRC_DEVICES_I2C_BIN_ARGS_H_

#include <lib/zx/result.h>

#include <string>
#include <vector>

namespace i2cutil {

constexpr char kI2cPathFormat[] = "/dev/class/i2c/%03u";

enum class I2cOp {
  Read,
  Write,
  Transact,
  Dump,
  List,
  Ping,
  Help,

  Undefined,
};

enum class TransactionType {
  Read,
  Write,
};

struct TransactionData {
  void Reset() {
    bytes.clear();
    count = 0;
  }
  std::vector<uint8_t> bytes;
  size_t count = 0;
  TransactionType type;
};

class Args {
 public:
  // Parse args from command line.
  static zx::result<Args> FromArgv(int argc, const char *argv[]);

  I2cOp Op() const { return op_; }
  const std::string &Path() { return path_; }
  std::vector<TransactionData> &Transactions() { return transactions_; }

 private:
  // Construct via `FromArgv`
  explicit Args() {}

  I2cOp op_ = I2cOp::Undefined;
  std::string path_;
  std::vector<TransactionData> transactions_;
};

}  // namespace i2cutil

#endif  // SRC_DEVICES_I2C_BIN_ARGS_H_
