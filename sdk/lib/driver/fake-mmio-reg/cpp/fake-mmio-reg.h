// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_MMIO_REG_CPP_FAKE_MMIO_REG_H_
#define LIB_DRIVER_FAKE_MMIO_REG_CPP_FAKE_MMIO_REG_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fit/function.h>
#include <lib/mmio-ptr/fake.h>

#include <vector>

namespace fake_mmio {

// Fakes a single MMIO register. This class is intended to be used with a fdf::MmioBuffer;
// operations on an instance of that class will be directed to the fake if the fake-mmio-reg library
// is a dependency of the test.
class FakeMmioReg {
 public:
  // Reads from the faked register. Returns the value set by the next expectation, or the default
  // value. The default is initially zero and can be set by calling ReadReturns() or Write(). This
  // method is expected to be called (indirectly) by the code under test.
  FakeMmioReg() {
    read_ = []() { return 0; };
    write_ = [](uint64_t value) {};
  }

  // Ses the read callback function. The function is invoked every time the faked register is read.
  void SetReadCallback(fit::function<uint64_t()> read) { read_ = std::move(read); }

  // Sets the write callback function. The function is invoked every time the faked register is
  // written.
  void SetWriteCallback(fit::function<void(uint64_t)> write) { write_ = std::move(write); }

  // Reads the faked register by calling the read callback and returning its value.
  uint64_t Read() { return read_(); }

  // Writes to the faked register. This method is expected to be called (indirectly) by the code
  // under test.
  void Write(uint64_t value) { write_(value); }

 private:
  // Callback functions for read and write operations.
  fit::function<void(uint64_t value)> write_;
  fit::function<uint64_t()> read_;
};

// Represents a region of fake MMIO registers. Each register is backed by a FakeMmioReg instance.
//
// Example:
// fake_mmio::FakeMmioRegRegion fake_registers(register_size, number_of_registers);
// fdf::MmioBuffer mmio_buffer(fake_registers.GetMmioBuffer());
// fake_registers[0].SetReadCallback(read_fn);
// fake_registers[0].SetWriteCallback(write_fn);
// SomeDriver dut(mmio_buffer);
//
// dut.DoSomeWork(); // backed by mmio_buffer.
class FakeMmioRegRegion {
 public:
  // Constructs a FakeMmioRegRegion backed by the given array. reg_size is the size of each
  // register in bytes, and reg_count is the total number of registers.
  FakeMmioRegRegion(size_t reg_size, size_t reg_count)
      : reg_size_(reg_size), reg_count_(reg_count) {
    ZX_ASSERT(reg_size_ > 0);
    regs_.resize(reg_count_);
  }

  // Accesses the FakeMmioReg at the given offset. Note that this is the byte offset of the region
  // of MMIO registers, not the index. The accessed FakeMmioReg will be at the index calculated by
  // |offset| / |reg_size_|
  const FakeMmioReg& operator[](size_t offset) const {
    ZX_ASSERT(offset / reg_size_ < reg_count_);
    return regs_[offset / reg_size_];
  }

  // Accesses the FakeMmioReg at the given offset. Note that this is the byte offset of the region
  // of MMIO registers, not the index. The accessed FakeMmioReg will be at the index calculated by
  // |offset| / |reg_size_|
  FakeMmioReg& operator[](size_t offset) {
    ZX_ASSERT(offset / reg_size_ < reg_count_);
    return regs_[offset / reg_size_];
  }

  // Constructs and returns a MmioBuffer object with a size that matches the FakeMmioReg.
  fdf::MmioBuffer GetMmioBuffer();

 private:
  static uint8_t Read8(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  static uint16_t Read16(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  static uint32_t Read32(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);
  static uint64_t Read64(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs);

  static void Write8(const void* ctx, const mmio_buffer_t& mmio, uint8_t val, zx_off_t offs);
  static void Write16(const void* ctx, const mmio_buffer_t& mmio, uint16_t val, zx_off_t offs);
  static void Write32(const void* ctx, const mmio_buffer_t& mmio, uint32_t val, zx_off_t offs);
  static void Write64(const void* ctx, const mmio_buffer_t& mmio, uint64_t val, zx_off_t offs);

  std::vector<FakeMmioReg> regs_;
  const size_t reg_size_;
  const size_t reg_count_;
};

}  // namespace fake_mmio

#endif  // LIB_DRIVER_FAKE_MMIO_REG_CPP_FAKE_MMIO_REG_H_
