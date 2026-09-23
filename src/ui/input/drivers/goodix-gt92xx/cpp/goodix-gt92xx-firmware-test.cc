// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/fake-i2c/fake-i2c.h>
#include <lib/fzl/vmo-mapper.h>

#include <array>

#include <gtest/gtest.h>

#include "goodix-gt92xx.h"
#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/predicates/status.h"

namespace {

bool enable_load_firmware = false;
bool corrupt_firmware_checksum = false;

}  // namespace

// Must not be in a namespace in order to override the weak implementation in fake_ddk.
zx_status_t load_firmware_from_driver(zx_driver_t* drv, zx_device_t* device, const char* path,
                                      zx_handle_t* fw, size_t* size) {
  constexpr uint8_t kFirmwareTestData[] = {0x52, 0xc0, 0xb3, 0x37, 0x84, 0x2c, 0xf0, 0xbc,
                                           0x88, 0xe7, 0xca, 0x28, 0x93, 0x9f, 0xed, 0x86,
                                           0xd6, 0x06, 0x4b, 0xb1, 0x72, 0x65, 0x45, 0x48,
                                           0x6d, 0xcf, 0x06, 0x86, 0xe7, 0xac, 0x39, 0x6f};

  constexpr uint8_t kFirmwareHeader[] = {
      0x00, 0x01, 0x60, 0x00,                          // Firmware size excluding header
      '9',  '2',  '9',  '3',  '\0', '\0', '\0', '\0',  // Product ID string
      0x61, 0x05,                                      // Firmware version number
  };
  constexpr size_t kFirmwareSize = sizeof(kFirmwareHeader) + 0x16000;

  if (!enable_load_firmware) {
    return ZX_ERR_NOT_FOUND;
  }

  zx::vmo firmware_vmo;
  fzl::VmoMapper firmware_mapper;
  zx_status_t status = firmware_mapper.CreateAndMap(
      kFirmwareSize, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &firmware_vmo);
  if (status != ZX_OK) {
    return status;
  }

  uint8_t* firmware = reinterpret_cast<uint8_t*>(firmware_mapper.start());
  memcpy(firmware, kFirmwareHeader, sizeof(kFirmwareHeader));

  constexpr size_t kSectionOffsets[] = {
      0x0000,  0x2000, 0x4000, 0x6000,   // SS51
      0xa800,  0xc800, 0xe800, 0x10800,  // Gwake
      0x15000,                           // DSP ISP
  };

  for (size_t offset : kSectionOffsets) {
    memcpy(firmware + sizeof(kFirmwareHeader) + offset, kFirmwareTestData,
           sizeof(kFirmwareTestData));
  }

  uint16_t checksum = 0;
  // Skip the first uint16 and put the checksum there after.
  for (size_t i = 1; i < (kFirmwareSize - sizeof(kFirmwareHeader)) / sizeof(checksum); i++) {
    checksum += be16toh(reinterpret_cast<uint16_t*>(firmware + sizeof(kFirmwareHeader))[i]);
  }
  checksum = htobe16(-checksum);
  if (corrupt_firmware_checksum) {
    checksum++;
  }

  memcpy(firmware + sizeof(kFirmwareHeader), &checksum, sizeof(checksum));

  *fw = firmware_vmo.release();
  *size = kFirmwareSize;
  return ZX_OK;
}

namespace goodix {

class FakeTouchDevice : public fake_i2c::FakeI2c {
 private:
  uint8_t product_info_[sizeof(uint32_t) + sizeof(uint16_t)] = {'9', '2', '9', '3', 0x04, 0x61};

 public:
  enum ControllerState {
    kIdle,
    kReadingDspIsp,
    kReadingGwake,
    kReadingSs51,
    kReadingDsp,
    kReadingBoot,
    kReadingBootIsp,
    kReadingLink,
    kReadingFirstSs51Section,
    kReady,
  };

  void SetFirmwareMessageInvalid() { firmware_message_ = 0; }
  void SetProductInfo(std::array<uint8_t, sizeof(product_info_)> product_info) {
    memcpy(product_info_, product_info.data(), sizeof(product_info_));
  }
  void SetCorruptSectionRead() { corrupt_section_read_ = true; }
  bool FirmwareWritten() const { return firmware_written_; }
  ControllerState CurrentState() const { return current_state_; }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    if (write_buffer_size < 2) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    const uint16_t address = (write_buffer[0] << 8) | write_buffer[1];
    write_buffer += 2;
    write_buffer_size -= 2;

    ControllerState next_state = current_state_;

    if (address == GT_REG_SW_RESET) {
      if (write_buffer_size == 0) {
        read_buffer[0] = sw_reset_;
        *read_buffer_size = 1;
      } else {
        sw_reset_ = write_buffer[0];
      }
    } else if (address == GT_REG_CPU_RESET && write_buffer_size == 1) {
      next_state = kReadingDspIsp;
    } else if (address == GT_REG_BOOT_CONTROL) {
      if (write_buffer_size == 0) {
        read_buffer[0] = 0;  // Always not busy
        *read_buffer_size = 1;
      } else if (write_buffer[0] == 0x00 && current_state_ == kReadingDspIsp) {
        // This assumes that Gwake is always written, even though it is technically optional.
        next_state = kReadingGwake;
      } else if (write_buffer[0] == 0xd && current_state_ == kReadingGwake) {
        next_state = kReadingSs51;
      } else if (write_buffer[0] == 0x4 && current_state_ == kReadingSs51) {
        next_state = kReadingDsp;
      } else if (write_buffer[0] == 0x5 && current_state_ == kReadingDsp) {
        next_state = kReadingBoot;
      } else if (write_buffer[0] == 0x6 && current_state_ == kReadingBoot) {
        next_state = kReadingBootIsp;
      } else if (write_buffer[0] == 0x7 && current_state_ == kReadingBootIsp) {
        next_state = kReadingLink;
      } else if (write_buffer[0] == 0x9 && current_state_ == kReadingLink) {
        next_state = kReadingFirstSs51Section;
      } else if (write_buffer[0] == 0x1) {
        // Boot ISP and Link are optional, but the process always ends with writing the first SS51
        // section.
        if (current_state_ == kReadingFirstSs51Section || current_state_ == kReadingBootIsp ||
            current_state_ == kReadingLink) {
          next_state = kReady;
        }
      }
    } else if (address == GT_REG_FIRMWARE && write_buffer_size == 0) {
      read_buffer[0] = firmware_message_;
      *read_buffer_size = 1;
    } else if (address == GT_REG_HW_INFO && write_buffer_size == 0) {
      memset(read_buffer, 0, sizeof(uint32_t));
      *read_buffer_size = sizeof(uint32_t);
    } else if (address == GT_REG_PRODUCT_INFO && write_buffer_size == 0) {
      memcpy(read_buffer, product_info_, sizeof(product_info_));
      *read_buffer_size = sizeof(product_info_);
    } else if ((address >= 0x9000 && address < 0xb000) || (address >= 0xc000)) {
      // This needs to be reset for the config check, at this point in the firmware download the
      // message value is no longer used.
      firmware_message_ = GT_FIRMWARE_MAGIC;

      // Map [0x9000, 0xb000) to [0x8000, 0xa000) -- that way the top bits can masked off to
      // get an offset.
      const uint16_t offset = (address ^ (address < 0xb000 ? 0x1000 : 0)) & 0x1fff;
      const size_t remaining_size = std::min<size_t>(sizeof(section_) - offset, 256);
      if (write_buffer_size == 0) {
        memcpy(read_buffer, &section_[offset], remaining_size);
        *read_buffer_size = remaining_size;
      } else if (write_buffer_size <= remaining_size) {
        firmware_written_ = true;
        memcpy(&section_[offset], write_buffer, write_buffer_size);
        if (corrupt_section_read_) {
          section_[offset]++;
        }
      } else {
        return ZX_ERR_IO;
      }
    } else if (write_buffer_size == 0) {
      *read_buffer = 0;
      *read_buffer_size = 1;
    }

    current_state_ = next_state;
    return ZX_OK;
  }

 private:
  uint8_t sw_reset_ = 0;
  uint8_t section_[0x2000];
  uint8_t firmware_message_ = GT_FIRMWARE_MAGIC;
  bool corrupt_section_read_ = false;
  bool firmware_written_ = false;

  ControllerState current_state_ = kIdle;
};

class Gt92xxTestDevice : public Gt92xxDevice {
 public:
  Gt92xxTestDevice(async_dispatcher_t* dispatcher, ddk::I2cChannel i2c,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> intr,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> reset, zx_device_t* parent)
      : Gt92xxDevice(parent, dispatcher, std::move(i2c), std::move(intr), std::move(reset)) {}

  void Running(bool run) { Gt92xxDevice::running_.store(run); }

  zx_status_t Init() { return Gt92xxDevice::Init(); }

  void Trigger() { irq_.trigger(0, zx::time_boot()); }

  zx_status_t StartThread() {
    EXPECT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq_));

    auto thunk = [](void* arg) -> int {
      return reinterpret_cast<Gt92xxTestDevice*>(arg)->Thread();
    };

    Running(true);
    int ret = thrd_create_with_name(&test_thread_, thunk, this, "gt92xx-test-thread");
    return (ret == thrd_success) ? ZX_OK : ZX_ERR_BAD_STATE;
  }

  zx_status_t StopThread() {
    Running(false);
    irq_.trigger(0, zx::time_boot());
    int ret = thrd_join(test_thread_, nullptr);
    return (ret == thrd_success) ? ZX_OK : ZX_ERR_BAD_STATE;
  }

  thrd_t test_thread_;
};

class GoodixTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(fidl_servers_loop_.StartThread("fidl-servers"));
    enable_load_firmware = true;
    fidl::ClientEnd reset_gpio_client = reset_gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
    fidl::ClientEnd intr_gpio_client = intr_gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    fidl::BindServer(fidl_servers_loop_.dispatcher(), std::move(endpoints.server), &i2c_);
    fake_parent_ = MockDevice::FakeRootParent();
    device_.emplace(nullptr, std::move(endpoints.client), std::move(intr_gpio_client),
                    std::move(reset_gpio_client), fake_parent_.get());
  }

  void TearDown() override {
    enable_load_firmware = false;
    corrupt_firmware_checksum = false;
    fidl_servers_loop_.Shutdown();
  }

  void VerifyInitialReset() {
    std::vector reset_states = reset_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(reset_states.size(), 2u);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, reset_states[0].sub_state);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 1}, reset_states[1].sub_state);

    std::vector intr_states = intr_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(intr_states.size(), 2u);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, intr_states[0].sub_state);
    ASSERT_EQ(fake_gpio::ReadSubState{}, intr_states[1].sub_state);
  }

  void VerifyEnteringUpdateMode(size_t reset_state_log_offset, size_t intr_state_log_offset) {
    std::vector reset_states = reset_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(reset_states.size(), reset_state_log_offset + 2);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, reset_states[reset_state_log_offset].sub_state);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 1},
              reset_states[reset_state_log_offset + 1].sub_state);

    std::vector intr_states = intr_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(intr_states.size(), intr_state_log_offset + 1);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, intr_states[intr_state_log_offset].sub_state);
  }

  void VerifyLeavingUpdateMode(size_t reset_state_log_offset, size_t intr_state_log_offset) {
    std::vector reset_states = reset_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(reset_states.size(), reset_state_log_offset + 3);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, reset_states[reset_state_log_offset].sub_state);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 1},
              reset_states[reset_state_log_offset + 1].sub_state);
    ASSERT_EQ(fake_gpio::ReadSubState{}, reset_states[reset_state_log_offset + 2].sub_state);

    std::vector intr_states = intr_gpio_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
    ASSERT_GE(intr_states.size(), intr_state_log_offset + 4);
    ASSERT_EQ(fake_gpio::ReadSubState{}, intr_states[intr_state_log_offset].sub_state);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0},
              intr_states[intr_state_log_offset + 1].sub_state);
    ASSERT_EQ(fake_gpio::WriteSubState{.value = 0},
              intr_states[intr_state_log_offset + 2].sub_state);
    ASSERT_EQ(fake_gpio::ReadSubState{}, intr_states[intr_state_log_offset + 3].sub_state);
  }

  void VerifyUpdateMode(size_t reset_state_log_offset, size_t intr_state_log_offset) {
    VerifyEnteringUpdateMode(reset_state_log_offset, intr_state_log_offset);
    VerifyLeavingUpdateMode(reset_state_log_offset + 2, intr_state_log_offset + 1);
  }

  async::Loop fidl_servers_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> reset_gpio_{
      fidl_servers_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> intr_gpio_{
      fidl_servers_loop_.dispatcher(), std::in_place};
  FakeTouchDevice i2c_;
  std::optional<Gt92xxTestDevice> device_;
  std::shared_ptr<MockDevice> fake_parent_;
};

TEST_F(GoodixTest, FirmwareTest) {
  EXPECT_OK(device_->Init());
  VerifyInitialReset();
  VerifyUpdateMode(2, 2);

  EXPECT_TRUE(i2c_.FirmwareWritten());
  EXPECT_EQ(i2c_.CurrentState(), FakeTouchDevice::kReady);
}

TEST_F(GoodixTest, FirmwareCurrent) {
  i2c_.SetProductInfo({'9', '2', '9', '3', 0x05, 0x61});

  EXPECT_OK(device_->Init());
  VerifyInitialReset();

  EXPECT_FALSE(i2c_.FirmwareWritten());
  EXPECT_EQ(i2c_.CurrentState(), FakeTouchDevice::kIdle);
}

TEST_F(GoodixTest, FirmwareNotApplicable) {
  // Wrong product ID
  i2c_.SetProductInfo({'9', '2', '9', '5', 0x04, 0x61});

  EXPECT_OK(device_->Init());
  VerifyInitialReset();

  EXPECT_FALSE(i2c_.FirmwareWritten());
  EXPECT_EQ(i2c_.CurrentState(), FakeTouchDevice::kIdle);
}

TEST_F(GoodixTest, ForceFirmwareUpdate) {
  // Wrong product ID
  i2c_.SetProductInfo({'9', '2', '9', '5', 0x04, 0x61});

  // Send an unknown firmware message so that the product ID/version check is skipped
  i2c_.SetFirmwareMessageInvalid();

  EXPECT_OK(device_->Init());
  VerifyInitialReset();

  VerifyUpdateMode(2, 2);

  EXPECT_TRUE(i2c_.FirmwareWritten());
  EXPECT_EQ(i2c_.CurrentState(), FakeTouchDevice::kReady);
}

TEST_F(GoodixTest, BadFirmwareChecksum) {
  corrupt_firmware_checksum = true;

  EXPECT_NE(device_->Init(), ZX_OK);
  VerifyInitialReset();

  EXPECT_FALSE(i2c_.FirmwareWritten());
  EXPECT_EQ(i2c_.CurrentState(), FakeTouchDevice::kIdle);
}

TEST_F(GoodixTest, ReadbackCheckFail) {
  i2c_.SetCorruptSectionRead();

  EXPECT_NE(device_->Init(), ZX_OK);
  VerifyInitialReset();
  VerifyEnteringUpdateMode(2, 2);

  EXPECT_TRUE(i2c_.FirmwareWritten());
  EXPECT_EQ(i2c_.CurrentState(), FakeTouchDevice::kReadingDspIsp);
}

}  // namespace goodix
