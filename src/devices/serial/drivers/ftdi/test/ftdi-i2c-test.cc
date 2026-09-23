// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ftdi-i2c.h"

#include <fidl/fuchsia.hardware.ftdi/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2c.businfo/cpp/wire.h>
#include <lib/ddk/debug.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <stdio.h>

#include <list>

#include <gtest/gtest.h>

#include "ftdi.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace ftdi_mpsse {

class FakeSerial : public ftdi_serial::FtdiSerial {
 public:
  void PushExpectedRead(std::vector<uint8_t> read) { expected_reads_.push_back(std::move(read)); }

  void PushExpectedWrite(std::vector<uint8_t> write) {
    expected_writes_.push_back(std::move(write));
  }

  void FailOnUnexpectedReadWrite(bool b) { unexpected_is_error_ = b; }

  zx_status_t Read(uint8_t* out_buf_buffer, size_t buf_size) override {
    uint8_t* out_buf = out_buf_buffer;
    if (expected_reads_.size() == 0) {
      if (unexpected_is_error_) {
        printf("Read with no expected read set\n");
        return ZX_ERR_INTERNAL;
      } else {
        return ZX_OK;
      }
    }
    std::vector<uint8_t>& read = expected_reads_.front();
    if (buf_size != read.size()) {
      printf("Read size mismatch (%lx != %lx\n", buf_size, read.size());
      return ZX_ERR_INTERNAL;
    }

    for (size_t i = 0; i < buf_size; i++) {
      out_buf[i] = read[i];
    }

    expected_reads_.pop_front();
    return ZX_OK;
  }

  zx_status_t Write(uint8_t* buf_buffer, size_t buf_size) override {
    const uint8_t* out_buf = buf_buffer;
    if (expected_writes_.size() == 0) {
      if (unexpected_is_error_) {
        printf("Write with no expected wrte set\n");
        return ZX_ERR_INTERNAL;
      } else {
        return ZX_OK;
      }
    }
    std::vector<uint8_t>& write = expected_writes_.front();
    if (buf_size != write.size()) {
      printf("Write size mismatch (0x%lx != 0x%lx\n", buf_size, write.size());
      return ZX_ERR_INTERNAL;
    }

    for (size_t i = 0; i < buf_size; i++) {
      if (out_buf[i] != write[i]) {
        printf("Write data mismatch index %ld (0x%x != 0x%x)\n", i, out_buf[i], write[i]);
        return ZX_ERR_INTERNAL;
      }
    }
    expected_writes_.pop_front();
    return ZX_OK;
  }

 private:
  bool unexpected_is_error_ = false;
  std::list<std::vector<uint8_t>> expected_reads_;
  std::list<std::vector<uint8_t>> expected_writes_;
};

class FtdiI2cTest : public testing::Test {
 public:
  void SetUp() override { fake_parent_ = MockDevice::FakeRootParent(); }

 protected:
  FtdiI2c FtdiBasicInit(void) {
    FtdiI2c::I2cLayout layout = {0, 1, 2};
    std::vector<FtdiI2c::I2cDevice> i2c_devices(1);
    i2c_devices[0].address = 0x3c;
    i2c_devices[0].vid = 0;
    i2c_devices[0].pid = 0;
    i2c_devices[0].did = 31;
    return FtdiI2c(fake_parent_.get(), &serial_, layout, i2c_devices);
  }

  std::shared_ptr<MockDevice> fake_parent_;
  FakeSerial serial_;
};

TEST_F(FtdiI2cTest, TrivialLifetimeTest) { FtdiI2c device = FtdiBasicInit(); }

TEST_F(FtdiI2cTest, DdkLifetimeTest) {
  FtdiI2c::I2cLayout layout = {0, 1, 2};
  std::vector<FtdiI2c::I2cDevice> i2c_devices(1);
  i2c_devices[0].address = 0x3c;
  i2c_devices[0].vid = 0;
  i2c_devices[0].pid = 0;
  i2c_devices[0].did = 31;
  FtdiI2c* device(new FtdiI2c(fake_parent_.get(), &serial_, layout, i2c_devices));

  // These Reads and Writes are to sync the device on bind.
  std::vector<uint8_t> first_write(1);
  first_write[0] = 0xAB;
  serial_.PushExpectedWrite(std::move(first_write));

  std::vector<uint8_t> first_read(2);
  first_read[0] = 0xFA;
  first_read[1] = 0xAB;

  serial_.PushExpectedRead(std::move(first_read));

  // Check that bind works.
  ASSERT_OK(device->Bind());
  auto* child = fake_parent_->GetLatestChild();
  child->InitOp();
  child->WaitUntilInitReplyCalled();
  device->DdkAsyncRemove();
  mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
}

TEST_F(FtdiI2cTest, DdkLifetimeFailedInit) {
  FtdiI2c::I2cLayout layout = {0, 1, 2};
  std::vector<FtdiI2c::I2cDevice> i2c_devices(1);
  i2c_devices[0].address = 0x3c;
  i2c_devices[0].vid = 0;
  i2c_devices[0].pid = 0;
  i2c_devices[0].did = 31;
  FtdiI2c* device(new FtdiI2c(fake_parent_.get(), &serial_, layout, i2c_devices));

  // These Reads and Writes are to sync the device on bind.
  std::vector<uint8_t> first_write(1);
  first_write[0] = 0xAB;
  serial_.PushExpectedWrite(std::move(first_write));

  // Set bad read data. This will cause the enable worker thread to fail.
  std::vector<uint8_t> first_read(2);
  first_read[0] = 0x00;
  first_read[1] = 0x00;

  serial_.PushExpectedRead(std::move(first_read));

  // Bind should spawn the worker thread which will fail the init.
  ASSERT_OK(device->Bind());
  auto* child = fake_parent_->GetLatestChild();

  child->InitOp();
  child->WaitUntilInitReplyCalled();
  ASSERT_EQ(ZX_ERR_INTERNAL, child->InitReplyCallStatus());

  mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
}

TEST_F(FtdiI2cTest, PingTest) {
  FtdiI2c device = FtdiBasicInit();
  std::vector<uint8_t> ping_data = {
      0x80, 0x3, 0x3,  0x82, 0x0, 0x0,  0x80, 0x1, 0x3,  0x82, 0x0,  0x0, 0x80, 0x0,  0x3,  0x82,
      0x0,  0x0, 0x11, 0x0,  0x0, 0x78, 0x80, 0x2, 0x3,  0x82, 0x0,  0x0, 0x22, 0x0,  0x11, 0x0,
      0x0,  0x0, 0x80, 0x2,  0x3, 0x82, 0x0,  0x0, 0x22, 0x0,  0x80, 0x0, 0x3,  0x82, 0x0,  0x0,
      0x80, 0x1, 0x3,  0x82, 0x0, 0x0,  0x80, 0x3, 0x3,  0x82, 0x0,  0x0, 0x87};
  serial_.PushExpectedWrite(std::move(ping_data));

  zx_status_t status = device.Ping(0x3c);
  ASSERT_OK(status);
}

TEST_F(FtdiI2cTest, ReadTest) {
  namespace fhi2cimpl = fuchsia_hardware_i2cimpl::wire;

  FtdiI2c device = FtdiBasicInit();

  serial_.FailOnUnexpectedReadWrite(false);
  std::vector<uint8_t> serial_read_data = {
      0x00,  // The ACK for writing bus address.
      0x00,  // The ACK for writing register value.
      0x00,  // The ACK for initiating a read.
      0xDE,  // The Value we will be reading out.
  };
  serial_.PushExpectedRead(std::move(serial_read_data));

  fdf::Arena arena('I2CI');

  fhi2cimpl::I2cImplOp op_list[2] = {};
  op_list[0].stop = false;
  uint8_t write_data = 0xAB;
  op_list[0].type = fhi2cimpl::I2cImplOpType::WithWriteData(arena, &write_data, &write_data + 1);

  op_list[1].stop = true;
  op_list[1].type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(1);

  auto [client_end, server_end] = fdf::Endpoints<fuchsia_hardware_i2cimpl::Device>::Create();

  fdf::ServerBinding binding(fdf::Dispatcher::GetCurrent()->get(), std::move(server_end), &device,
                             fidl::kIgnoreBindingClosure);

  fdf::WireClient client(std::move(client_end), fdf::Dispatcher::GetCurrent()->get());

  client.buffer(arena)
      ->Transact(fidl::VectorView<fhi2cimpl::I2cImplOp>::FromExternal(op_list, 2))
      .Then([](fdf::WireUnownedResult<fuchsia_hardware_i2cimpl::Device::Transact>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        ASSERT_EQ(result->value()->read.size(), 1u);
        ASSERT_EQ(result->value()->read[0].data.size(), 1u);
        EXPECT_EQ(result->value()->read[0].data[0], 0xDE);
        mock_ddk::GetDriverRuntime()->Quit();
      });
  mock_ddk::GetDriverRuntime()->Run();
}

TEST_F(FtdiI2cTest, NackReadTest) {
  namespace fhi2cimpl = fuchsia_hardware_i2cimpl::wire;

  FtdiI2c device = FtdiBasicInit();

  serial_.FailOnUnexpectedReadWrite(false);
  std::vector<uint8_t> serial_read_data = {
      0x01,  // The NACK for writing bus address.
      0x01,  // The NACK for writing register value.
      0x01,  // The NACK for initiating a read.
      0x00,  // The Value we will be reading out.
  };
  serial_.PushExpectedRead(std::move(serial_read_data));

  fdf::Arena arena('I2CI');

  fhi2cimpl::I2cImplOp op_list[2] = {};
  op_list[0].stop = false;
  uint8_t write_data = 0xAB;
  op_list[0].type = fhi2cimpl::I2cImplOpType::WithWriteData(arena, &write_data, &write_data + 1);

  op_list[1].stop = true;
  op_list[1].type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(1);

  auto [client_end, server_end] = fdf::Endpoints<fuchsia_hardware_i2cimpl::Device>::Create();

  fdf::ServerBinding binding(fdf::Dispatcher::GetCurrent()->get(), std::move(server_end), &device,
                             fidl::kIgnoreBindingClosure);

  fdf::WireClient client(std::move(client_end), fdf::Dispatcher::GetCurrent()->get());

  client.buffer(arena)
      ->Transact(fidl::VectorView<fhi2cimpl::I2cImplOp>::FromExternal(op_list, 2))
      .Then([](fdf::WireUnownedResult<fuchsia_hardware_i2cimpl::Device::Transact>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_INTERNAL);
        mock_ddk::GetDriverRuntime()->Quit();
      });
  mock_ddk::GetDriverRuntime()->Run();
}

// TODO(b/333883481): Test that the ftdi-i2c driver is correctly forwarding its I2C bus metadata
// once the driver is converted to DFv2.
// TEST_F(FtdiI2cTest, MetadataTest) {
//   FtdiI2c::I2cLayout layout = {0, 1, 2};
//   std::vector<FtdiI2c::I2cDevice> i2c_devices(1);
//   i2c_devices[0].address = 0x3c;
//   i2c_devices[0].vid = 0;
//   i2c_devices[0].pid = 0;
//   i2c_devices[0].did = 31;
//   FtdiI2c* device(new FtdiI2c(fake_parent_.get(), &serial_, layout, i2c_devices));

//   std::vector<uint8_t> first_write(1);
//   first_write[0] = 0xAB;
//   serial_.PushExpectedWrite(std::move(first_write));

//   std::vector<uint8_t> first_read(2);
//   first_read[0] = 0xFA;
//   first_read[1] = 0xAB;

//   serial_.PushExpectedRead(std::move(first_read));

//   // Check that bind works.
//   ASSERT_OK(device->Bind());

//   auto* child = fake_parent_->GetLatestChild();
//   child->InitOp();
//   child->WaitUntilInitReplyCalled();
//   EXPECT_OK(child->InitReplyCallStatus());

//   auto decoded = ddk::GetEncodedMetadata<fuchsia_hardware_i2c_businfo::wire::I2CBusMetadata>(
//       child, DEVICE_METADATA_I2C_CHANNELS);
//   ASSERT_TRUE(decoded.is_ok());

//   ASSERT_TRUE(decoded->has_bus_id());
//   EXPECT_EQ(decoded->bus_id(), 0u);

//   ASSERT_TRUE(decoded->has_channels());
//   ASSERT_EQ(decoded->channels().count(), 1u);

//   ASSERT_TRUE(decoded->channels()[0].has_address());
//   ASSERT_TRUE(decoded->channels()[0].has_vid());
//   ASSERT_TRUE(decoded->channels()[0].has_pid());
//   ASSERT_TRUE(decoded->channels()[0].has_did());

//   // Should match the I2cDevice passed above.
//   EXPECT_EQ(decoded->channels()[0].address(), 0x3c);
//   EXPECT_EQ(decoded->channels()[0].vid(), 0u);
//   EXPECT_EQ(decoded->channels()[0].pid(), 0u);
//   EXPECT_EQ(decoded->channels()[0].did(), 31u);
// }

}  // namespace ftdi_mpsse
