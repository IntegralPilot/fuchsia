// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_MOCK_MMIO_CPP_REGISTER_H_
#define LIB_DRIVER_MOCK_MMIO_CPP_REGISTER_H_

#include <lib/driver/mmio/cpp/mmio.h>

#include <memory>
#include <vector>

namespace mock_mmio {

// Mocks a single MMIO register. This class is intended to be used with a fdf::MmioBuffer;
// operations on an instance of that class will be directed to the mock if the mock-mmio library
// is a dependency of the test.
//
// Each ExpectRead() and ExpectWrite() call adds to the Register's list of expected transactions
// in FIFO order and must be added before the actual read or write call. When the Register receives
// a Read() or Write() call, it removes the next expected transaction in the list and verifies that
// it matches the Read() or Write() call.
//
// At the end of the test, VerifyAndClear() should be called to verify that the Register has no
// outstanding expected transactions.
//
class Register {
 public:
  // Reads from the mocked register. Returns the value set by the next expectation, or the default
  // value. The default is initially zero and can be set by calling ReadReturns() or Write(). This
  // method is expected to be called (indirectly) by the code under test.
  uint64_t Read();

  // Writes to the mocked register. This method is expected to be called (indirectly) by the code
  // under test.
  void Write(uint64_t value);

  // Matches a register read and returns the specified value.
  Register& ExpectRead(uint64_t value);

  // Matches a register read and returns the default value.
  Register& ExpectRead();

  // Sets the default register read value.
  Register& ReadReturns(uint64_t value);

  // Matches a register write with the specified value.
  Register& ExpectWrite(uint64_t value);

  // Matches any register write.
  Register& ExpectWrite();

  // Removes and ignores all expectations and resets the default read value.
  void Clear();

  // Removes all expectations and resets the default value. The presence of any outstanding
  // expectations causes a test failure.
  void VerifyAndClear();

 private:
  struct MmioExpectation {
    enum class Match { kEqual, kAny } match;
    uint64_t value;
  };

  uint64_t last_value_ = 0;

  size_t read_expectations_index_ = 0;
  std::vector<MmioExpectation> read_expectations_;

  size_t write_expectations_index_ = 0;
  std::vector<MmioExpectation> write_expectations_;
};

}  // namespace mock_mmio

#endif  // LIB_DRIVER_MOCK_MMIO_CPP_REGISTER_H_
