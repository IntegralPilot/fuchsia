// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mock-mmio/cpp/region.h>

namespace mock_mmio {

// Reads from the mocked register. Returns the value set by the next expectation, or the default
// value. The default is initially zero and can be set by calling ReadReturns() or Write(). This
// method is expected to be called (indirectly) by the code under test.
uint64_t Register::Read() {
  if (read_expectations_index_ >= read_expectations_.size()) {
    return last_value_;
  }

  MmioExpectation& exp = read_expectations_[read_expectations_index_++];
  if (exp.match == MmioExpectation::Match::kAny) {
    return last_value_;
  }

  return last_value_ = exp.value;
}

// Writes to the mocked register. This method is expected to be called (indirectly) by the code
// under test.
void Register::Write(uint64_t value) {
  last_value_ = value;

  if (write_expectations_index_ >= write_expectations_.size()) {
    return;
  }

  Register::MmioExpectation& exp = write_expectations_[write_expectations_index_++];
  if (exp.match != MmioExpectation::Match::kAny) {
    ZX_ASSERT(exp.value == value);
  }
}

// Matches a register read and returns the specified value.
Register& Register::ExpectRead(uint64_t value) {
  read_expectations_.push_back(
      MmioExpectation{.match = MmioExpectation::Match::kEqual, .value = value});

  return *this;
}

// Matches a register read and returns the default value.
Register& Register::ExpectRead() {
  read_expectations_.push_back(MmioExpectation{.match = MmioExpectation::Match::kAny, .value = 0});

  return *this;
}

// Sets the default register read value.
Register& Register::ReadReturns(uint64_t value) {
  last_value_ = value;
  return *this;
}

// Matches a register write with the specified value.
Register& Register::ExpectWrite(uint64_t value) {
  write_expectations_.push_back(
      MmioExpectation{.match = MmioExpectation::Match::kEqual, .value = value});

  return *this;
}

// Matches any register write.
Register& Register::ExpectWrite() {
  write_expectations_.push_back(MmioExpectation{.match = MmioExpectation::Match::kAny, .value = 0});

  return *this;
}

void Register::Clear() {
  last_value_ = 0;

  read_expectations_index_ = 0;
  while (read_expectations_.size() > 0) {
    read_expectations_.pop_back();
  }

  write_expectations_index_ = 0;
  while (write_expectations_.size() > 0) {
    write_expectations_.pop_back();
  }
}

void Register::VerifyAndClear() {
  ZX_ASSERT(read_expectations_index_ >= read_expectations_.size());
  ZX_ASSERT(write_expectations_index_ >= write_expectations_.size());
  Clear();
}

}  // namespace mock_mmio
