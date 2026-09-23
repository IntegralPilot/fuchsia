// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ti-tca6408a.h"

#include <fidl/fuchsia.hardware.pinimpl/cpp/driver/fidl.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/internal/driver_lifecycle.h>
#include <lib/driver/testing/cpp/internal/test_environment.h>
#include <lib/driver/testing/cpp/test_node.h>
#include <lib/fake-i2c/fake-i2c.h>

#include "src/lib/testing/predicates/status.h"

namespace gpio {

class FakeTiTca6408aDevice : public fake_i2c::FakeI2c {
 public:
  uint8_t input_port() const { return input_port_; }
  void set_input_port(uint8_t input_port) { input_port_ = input_port; }
  uint8_t output_port() const { return output_port_; }
  uint8_t polarity_inversion() const { return polarity_inversion_; }
  uint8_t configuration() const { return configuration_; }

  fuchsia_hardware_i2c::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_i2c::Service::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    if (write_buffer_size > 2) {
      return ZX_ERR_IO;
    }

    const uint8_t address = write_buffer[0];

    uint8_t* reg = nullptr;
    switch (address) {
      case 0:
        reg = &input_port_;
        break;
      case 1:
        reg = &output_port_;
        break;
      case 2:
        reg = &polarity_inversion_;
        break;
      case 3:
        reg = &configuration_;
        break;
      default:
        return ZX_ERR_IO;
    };

    if (write_buffer_size == 1) {
      *read_buffer = *reg;
      *read_buffer_size = 1;
    } else {
      *reg = write_buffer[1];
    }

    return ZX_OK;
  }

 private:
  uint8_t input_port_ = 0;
  uint8_t output_port_ = 0b1111'1111;
  uint8_t polarity_inversion_ = 0;
  uint8_t configuration_ = 0b1111'1111;
};

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher()), "pdev");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_i2c::Service>(
          std::move(i2c_.GetInstanceHandler()), "i2c");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  FakeTiTca6408aDevice& i2c() { return i2c_; }

 private:
  FakeTiTca6408aDevice i2c_;
  fdf_fake::FakePDev pdev_;
};

class TiTca6408aTestConfig {
 public:
  using DriverType = TiTca6408aDevice;
  using EnvironmentType = Environment;
};

class TiTca6408aTest : public testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver());
    zx::result clock_impl = driver_test_.Connect<fuchsia_hardware_pinimpl::Service::Device>();
    ASSERT_OK(clock_impl);
    gpio_.Bind(std::move(clock_impl.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void RunInI2cContext(fit::callback<void(FakeTiTca6408aDevice&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](Environment& environment) mutable {
          callback(environment.i2c());
        });
  }

  fdf::WireSyncClient<fuchsia_hardware_pinimpl::PinImpl> gpio_;

 private:
  fdf_testing::BackgroundDriverTest<TiTca6408aTestConfig> driver_test_;
};

TEST_F(TiTca6408aTest, SetBufferMode) {
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) {
    EXPECT_EQ(i2c.output_port(), 0b1111'1111);
    EXPECT_EQ(i2c.configuration(), 0b1111'1111);
  });

  fdf::Arena arena('TEST');
  {
    auto result =
        gpio_.buffer(arena)->SetBufferMode(0, fuchsia_hardware_gpio::BufferMode::kOutputLow);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) {
    EXPECT_EQ(i2c.output_port(), 0b1111'1110);
    EXPECT_EQ(i2c.configuration(), 0b1111'1110);
  });

  {
    auto result = gpio_.buffer(arena)->SetBufferMode(0, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) {
    EXPECT_EQ(i2c.output_port(), 0b1111'1110);
    EXPECT_EQ(i2c.configuration(), 0b1111'1111);
  });

  {
    auto result =
        gpio_.buffer(arena)->SetBufferMode(5, fuchsia_hardware_gpio::BufferMode::kOutputLow);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) {
    EXPECT_EQ(i2c.output_port(), 0b1101'1110);
    EXPECT_EQ(i2c.configuration(), 0b1101'1111);
  });

  {
    auto result = gpio_.buffer(arena)->SetBufferMode(5, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) {
    EXPECT_EQ(i2c.output_port(), 0b1101'1110);
    EXPECT_EQ(i2c.configuration(), 0b1111'1111);
  });

  {
    auto result =
        gpio_.buffer(arena)->SetBufferMode(5, fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) {
    EXPECT_EQ(i2c.output_port(), 0b1111'1110);
    EXPECT_EQ(i2c.configuration(), 0b1101'1111);
  });

  {
    auto result =
        gpio_.buffer(arena)->SetBufferMode(5, fuchsia_hardware_gpio::BufferMode::kOutputLow);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
}

TEST_F(TiTca6408aTest, Read) {
  RunInI2cContext([](FakeTiTca6408aDevice& i2c) { i2c.set_input_port(0x55); });

  fdf::Arena arena('TEST');
  {
    auto result = gpio_.buffer(arena)->Read(0);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 1);
  };

  {
    auto result = gpio_.buffer(arena)->Read(3);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0);
  };

  {
    auto result = gpio_.buffer(arena)->Read(4);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 1);
  };

  {
    auto result = gpio_.buffer(arena)->Read(7);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0);
  };

  {
    auto result = gpio_.buffer(arena)->Read(5);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
}

TEST_F(TiTca6408aTest, InvalidArgs) {
  fdf::Arena arena('TEST');
  {
    auto result = gpio_.buffer(arena)->Read(7);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  };
  {
    auto result = gpio_.buffer(arena)->Read(8);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  };
  {
    auto result =
        gpio_.buffer(arena)->SetBufferMode(100, fuchsia_hardware_gpio::BufferMode::kOutputLow);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  };
  {
    auto result =
        gpio_.buffer(arena)->SetBufferMode(101, fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  };
}

}  // namespace gpio
