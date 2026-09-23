/*
 * Copyright (c) 2019 The Fuchsia Authors
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
 * SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
 * OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
 * CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sdio/sdio.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fuchsia/hardware/sdio/c/banjo.h>
#include <fuchsia/hardware/sdio/cpp/banjo-mock.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <tuple>

#include <sdk/lib/driver/testing/cpp/driver_runtime.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/bus.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/common.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/test/stub_device.h"
#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"

// These numbers come from real bugs.
#define NOT_ALIGNED_SIZE 1541
#define ALIGNED_SIZE 1544

// This is required to use ddk::MockSdio.
bool operator==(const sdio_rw_txn_t& lhs, const sdio_rw_txn_t& rhs) { return false; }

zx::result<fuchsia_wlan_broadcom::WifiConfig> get_wifi_metadata(struct brcmf_bus* bus) {
  return bus->bus_priv.sdio->drvr->device->GetWifiConfig();
}

namespace {

class FakeSdioDevice : public wlan::brcmfmac::StubDevice {
 public:
  ~FakeSdioDevice() {
    libsync::Completion shutdown_complete;
    Shutdown([&] { shutdown_complete.Signal(); });
    shutdown_complete.Wait();
  }

  zx::result<fuchsia_wlan_broadcom::WifiConfig> GetWifiConfig() override {
    // Provide a fake implementation for this metadata.
    return zx::ok(fuchsia_wlan_broadcom::WifiConfig{{
        .oob_irq_mode = ZX_INTERRUPT_MODE_LEVEL_LOW,
        .clm_needed = true,
    }});
  }
};

class MockSdio : public ddk::MockSdio {
 public:
  // Override these methods because the generated methods in ddk::MockSdio require that
  // out_read_byte is non-null, which is only the case for reads. For writes it can/should be null.
  zx_status_t SdioDoRwByte(bool write, uint32_t addr, uint8_t write_byte,
                           uint8_t* out_read_byte) override {
    std::tuple<zx_status_t, uint8_t> ret = mock_do_rw_byte_.Call(write, addr, write_byte);
    if (out_read_byte) {
      *out_read_byte = std::get<1>(ret);
    }
    return std::get<0>(ret);
  }

  zx_status_t SdioDoVendorControlRwByte(bool write, uint8_t addr, uint8_t write_byte,
                                        uint8_t* out_read_byte) override {
    auto ret = mock_do_vendor_control_rw_byte_.Call(write, addr, write_byte);
    if (out_read_byte != nullptr) {
      *out_read_byte = std::get<1>(ret);
    }

    return std::get<0>(ret);
  }
};

class FakeSdioBus {
 public:
  static constexpr size_t kVmoSize = 4096;
  static constexpr size_t kVmoOffset = 0;
  static constexpr uint8_t kVmoId = 0;
  static constexpr uint32_t kFrameSize = 1500;
  static constexpr uint8_t kPortId = 0;

  static zx_status_t Create(std::unique_ptr<FakeSdioBus>* bus) {
    std::unique_ptr<FakeSdioBus> ptr(new FakeSdioBus());

    zx_status_t status = zx::vmo::create(kVmoSize, 0, &ptr->vmo_);
    if (status != ZX_OK) {
      return status;
    }

    zx_vaddr_t addr = 0;
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, kVmoOffset, ptr->vmo_,
                                        kVmoOffset, kVmoSize, &addr);
    if (status != ZX_OK) {
      return status;
    }
    ptr->mapped_vmo_addr_ = addr;

    {
      std::lock_guard lock(ptr->bus_.rx_tx_data.tx_space);
      wlan::drivers::components::Frame tx_frame(
          &ptr->bus_.rx_tx_data.tx_space, kVmoId, kVmoOffset, 0,
          reinterpret_cast<uint8_t*>(ptr->mapped_vmo_addr_.value()), kFrameSize, kPortId);
      ptr->bus_.rx_tx_data.tx_space.Store(std::move(tx_frame));
    }

    *bus = std::move(ptr);

    return ZX_OK;
  }

  ~FakeSdioBus() {
    if (mapped_vmo_addr_.has_value()) {
      zx_status_t status = zx::vmar::root_self()->unmap(mapped_vmo_addr_.value(), kVmoSize);
      if (status != ZX_OK) {
        BRCMF_ERR("Failed to unmap VMO: %s", zx_status_get_string(status));
      }
    }
  }

  void ExpectDoRwTxn(MockSdio& sdio, zx_status_t return_status, uint32_t addr, uint32_t size,
                     bool incr, bool write) {
    sdio.mock_do_rw_txn().ExpectCallWithMatcher([=](sdio_rw_txn_t txn) {
      EXPECT_EQ(txn.addr, addr);
      EXPECT_EQ(txn.write, write);
      EXPECT_EQ(txn.incr, incr);
      EXPECT_EQ(txn.buffers_count, 1);
      EXPECT_EQ(txn.buffers_list[0].offset, kVmoOffset);
      EXPECT_EQ(txn.buffers_list[0].size, size);
      EXPECT_EQ(txn.buffers_list[0].type, SDMMC_BUFFER_TYPE_VMO_ID);
      EXPECT_EQ(txn.buffers_list[0].buffer.vmo_id, kVmoId);

      return std::tuple<zx_status_t>{return_status};
    });
  }

  void ExpectWriteByte(MockSdio& sdio, zx_status_t return_status, uint32_t expected_addr,
                       uint8_t expected_write_byte) {
    sdio.mock_do_rw_byte().ExpectCallWithMatcher(
        [=](bool write, uint32_t actual_addr, uint8_t actual_write_byte) {
          EXPECT_EQ(true, write);
          EXPECT_EQ(expected_addr, actual_addr);
          EXPECT_EQ(expected_write_byte, actual_write_byte);
          return std::tuple<zx_status_t, uint8_t>{return_status, 0};
        });
  }

  brcmf_sdio* get() { return &bus_; }

 private:
  FakeSdioBus() = default;

  brcmf_sdio bus_ = {};
  zx::vmo vmo_;
  std::optional<zx_vaddr_t> mapped_vmo_addr_;
};

class SdioTest : public zxtest::Test {
  fdf_testing::ScopedGlobalLogger logging_;

 private:
  // Create a testing driver runtime to allow for the creation of fdf dispatchers.
  fdf_testing::DriverRuntime driver_runtime_;
};

TEST_F(SdioTest, IntrRegisterUnregister) {
  FakeSdioDevice device;
  brcmf_sdio_dev sdio_dev = {};
  sdio_func func1 = {};
  MockSdio sdio1;
  MockSdio sdio2;
  brcmf_bus bus_if = {};
  brcmf_mp_device settings = {};
  brcmf_sdio_pd sdio_settings = {};
  const struct brcmf_bus_ops sdio_bus_ops = {
      .get_wifi_metadata = get_wifi_metadata,
  };

  async::Loop fidl_loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> wifi_gpio{fidl_loop.dispatcher(),
                                                                     std::in_place};
  EXPECT_OK(fidl_loop.StartThread("fidl-servers"));

  sdio_dev.func1 = &func1;
  auto wifi_gpio_client = wifi_gpio.SyncCall(&fake_gpio::FakeGpio::Connect);
  sdio_dev.fidl_gpios[WIFI_OOB_IRQ_GPIO_INDEX].Bind(std::move(wifi_gpio_client));
  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.sdio_proto_fn2 = *sdio2.GetProto();
  sdio_dev.drvr = device.drvr();
  bus_if.bus_priv.sdio = &sdio_dev;
  bus_if.ops = &sdio_bus_ops;
  sdio_dev.bus_if = &bus_if;
  sdio_dev.settings = &settings;
  sdio_dev.settings->bus.sdio = &sdio_settings;

  sdio1.ExpectEnableFnIntr(ZX_OK).ExpectDoVendorControlRwByte(
      ZX_OK, true, SDIO_CCCR_BRCM_SEPINT, SDIO_CCCR_BRCM_SEPINT_MASK | SDIO_CCCR_BRCM_SEPINT_OE, 0);
  sdio2.ExpectEnableFnIntr(ZX_OK);

  EXPECT_OK(brcmf_sdiod_intr_register(&sdio_dev, false));

  auto wifi_gpio_states = wifi_gpio.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
  ASSERT_GT(wifi_gpio_states.size(), 0);
  EXPECT_TRUE(std::holds_alternative<fake_gpio::ReadSubState>(wifi_gpio_states.back().sub_state));
  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();

  // The interrupt handle handed out by FakeGpio is a duplicated handle. Closing the handle in
  // sdio_dev is not enough to make the IRQ thread stop. The handle in FakeGpio also needs to close.
  // Do this by destroying the FakeGpio instance.
  wifi_gpio.reset();

  // Unregister the interrupt.
  sdio1.ExpectDoVendorControlRwByte(ZX_OK, true, SDIO_CCCR_BRCM_SEPINT, 0, 0);
  sdio1.ExpectDisableFnIntr(ZX_OK);
  sdio2.ExpectDisableFnIntr(ZX_OK);

  brcmf_sdiod_intr_unregister(&sdio_dev);

  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();
}

TEST_F(SdioTest, IntrRegisterFwReload) {
  FakeSdioDevice device;
  brcmf_sdio_dev sdio_dev = {};
  sdio_func func1 = {};
  MockSdio sdio1;
  MockSdio sdio2;
  brcmf_bus bus_if = {};
  brcmf_mp_device settings = {};
  brcmf_sdio_pd sdio_settings = {};
  const struct brcmf_bus_ops sdio_bus_ops = {
      .get_wifi_metadata = get_wifi_metadata,
  };

  async::Loop fidl_loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> wifi_gpio{fidl_loop.dispatcher(),
                                                                     std::in_place};
  EXPECT_OK(fidl_loop.StartThread("fidl-servers"));
  sdio_dev.func1 = &func1;
  auto wifi_gpio_client = wifi_gpio.SyncCall(&fake_gpio::FakeGpio::Connect);
  sdio_dev.fidl_gpios[WIFI_OOB_IRQ_GPIO_INDEX].Bind(std::move(wifi_gpio_client));
  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.sdio_proto_fn2 = *sdio2.GetProto();
  sdio_dev.drvr = device.drvr();
  bus_if.bus_priv.sdio = &sdio_dev;
  bus_if.ops = &sdio_bus_ops;
  sdio_dev.bus_if = &bus_if;
  sdio_dev.settings = &settings;
  sdio_dev.settings->bus.sdio = &sdio_settings;

  sdio1.ExpectEnableFnIntr(ZX_OK).ExpectDoVendorControlRwByte(
      ZX_OK, true, SDIO_CCCR_BRCM_SEPINT, SDIO_CCCR_BRCM_SEPINT_MASK | SDIO_CCCR_BRCM_SEPINT_OE, 0);
  sdio2.ExpectEnableFnIntr(ZX_OK);

  EXPECT_OK(brcmf_sdiod_intr_register(&sdio_dev, true));

  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();
  zx_handle_close(sdio_dev.irq_handle);
}

TEST_F(SdioTest, IntrRegisterUnregisterNoMetadata) {
  FakeSdioDevice device;
  brcmf_sdio_dev sdio_dev = {};
  sdio_func func1 = {};
  brcmf_bus bus_if = {};
  brcmf_mp_device settings = {};
  brcmf_sdio_pd sdio_settings = {};
  // If the get metadata call fails with ZX_ERR_NOT_FOUND the register and unregister calls will
  // behave differently.
  const struct brcmf_bus_ops sdio_bus_ops = {
      .get_wifi_metadata = [](brcmf_bus*) -> zx::result<fuchsia_wlan_broadcom::WifiConfig> {
        return zx::error(ZX_ERR_NOT_FOUND);
      },
  };

  MockSdio sdio1;
  MockSdio sdio2;
  sdio_dev.func1 = &func1;
  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.sdio_proto_fn2 = *sdio2.GetProto();
  sdio_dev.drvr = device.drvr();
  bus_if.ops = &sdio_bus_ops;
  sdio_dev.bus_if = &bus_if;
  sdio_dev.settings = &settings;
  sdio_dev.settings->bus.sdio = &sdio_settings;

  // Register interrupt without metadata.
  sdio1.ExpectEnableFnIntr(ZX_OK);
  sdio2.ExpectEnableFnIntr(ZX_OK);

  EXPECT_OK(brcmf_sdiod_intr_register(&sdio_dev, false));

  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();

  // Unregister interrupt without metadata.
  sdio1.ExpectDisableFnIntr(ZX_OK);
  sdio2.ExpectDisableFnIntr(ZX_OK);

  brcmf_sdiod_intr_unregister(&sdio_dev);

  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();
}

TEST_F(SdioTest, VendorControl) {
  brcmf_sdio_dev sdio_dev = {};

  MockSdio sdio1;
  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();

  sdio1.ExpectDoVendorControlRwByte(ZX_ERR_IO, false, 0xf0, 0, 0xab)
      .ExpectDoVendorControlRwByte(ZX_OK, false, 0xf3, 0, 0x12)
      .ExpectDoVendorControlRwByte(ZX_ERR_BAD_STATE, true, 0xff, 0x55, 0)
      .ExpectDoVendorControlRwByte(ZX_ERR_TIMED_OUT, true, 0xfd, 0x79, 0);

  zx_status_t status;

  EXPECT_EQ(brcmf_sdiod_vendor_control_rb(&sdio_dev, 0xf0, &status), 0xab);
  EXPECT_EQ(status, ZX_ERR_IO);
  EXPECT_EQ(brcmf_sdiod_vendor_control_rb(&sdio_dev, 0xf3, nullptr), 0x12);

  brcmf_sdiod_vendor_control_wb(&sdio_dev, 0xff, 0x55, nullptr);
  brcmf_sdiod_vendor_control_wb(&sdio_dev, 0xfd, 0x79, &status);
  EXPECT_EQ(status, ZX_ERR_TIMED_OUT);

  sdio1.VerifyAndClear();
}

TEST_F(SdioTest, Transfer) {
  FakeSdioDevice device;
  std::unique_ptr<FakeSdioBus> sdio_bus;
  ASSERT_OK(FakeSdioBus::Create(&sdio_bus));
  brcmf_sdio_dev sdio_dev = {.drvr = device.drvr(), .bus = sdio_bus->get()};

  MockSdio sdio1;
  MockSdio sdio2;
  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.sdio_proto_fn2 = *sdio2.GetProto();

  sdio_bus->ExpectDoRwTxn(sdio1, ZX_OK, 0x458ef43b, FakeSdioBus::kFrameSize / 2, true, true);
  sdio_bus->ExpectDoRwTxn(sdio2, ZX_OK, 0x216977b9, FakeSdioBus::kFrameSize, true, true);

  uint8_t some_data[FakeSdioBus::kFrameSize] = {};

  EXPECT_OK(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x458ef43b, some_data,
                              FakeSdioBus::kFrameSize / 2, false));
  EXPECT_OK(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn2, 0x216977b9, some_data,
                              FakeSdioBus::kFrameSize, false));

  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();
}

TEST_F(SdioTest, IoAbort) {
  brcmf_sdio_dev sdio_dev = {};

  MockSdio sdio1;
  MockSdio sdio2;
  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.sdio_proto_fn2 = *sdio2.GetProto();

  sdio1.ExpectIoAbort(ZX_OK);
  sdio2.ExpectIoAbort(ZX_OK).ExpectIoAbort(ZX_OK).ExpectIoAbort(ZX_OK);

  EXPECT_OK(brcmf_sdiod_abort(&sdio_dev, 1));
  EXPECT_OK(brcmf_sdiod_abort(&sdio_dev, 2));
  EXPECT_OK(brcmf_sdiod_abort(&sdio_dev, 0));
  EXPECT_OK(brcmf_sdiod_abort(&sdio_dev, 200));

  sdio1.VerifyAndClear();
  sdio2.VerifyAndClear();
}

TEST_F(SdioTest, RamRw) {
  FakeSdioDevice device;
  std::unique_ptr<FakeSdioBus> sdio_bus;
  ASSERT_OK(FakeSdioBus::Create(&sdio_bus));
  brcmf_sdio_dev sdio_dev = {.drvr = device.drvr(), .bus = sdio_bus->get()};

  sdio_func func1 = {};
  pthread_mutex_init(&func1.lock, nullptr);

  MockSdio sdio1;

  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.func1 = &func1;

  sdio_bus->ExpectDoRwTxn(sdio1, ZX_OK, 0x0000ffe0, 0x00000020, true, true);
  sdio_bus->ExpectWriteByte(sdio1, ZX_OK, 0x0001000a, 0x80);
  sdio_bus->ExpectWriteByte(sdio1, ZX_OK, 0x0001000b, 0x00);
  sdio_bus->ExpectWriteByte(sdio1, ZX_OK, 0x0001000c, 0x00);
  sdio_bus->ExpectDoRwTxn(sdio1, ZX_OK, 0x00008000, 0x00000020, true, true);

  /* In this test the address is set to 0x000007fe0, and when running, this function
   will chunk the data which is originally 0x40 bytes big into two pieces to align
   the next transfer address to SBSDIO_SB_OFT_ADDR_LIMIT, which is 0x8000, each one
   is 0x20 bytes big. The first line above corresponding to the first piece, and the
   fifth line is the second piece, middle three are byte writes made in
   brcmf_sdiod_set_backplane_window().
   */
  uint8_t some_data[128] = {};
  EXPECT_OK(brcmf_sdiod_ramrw(&sdio_dev, true, 0x00007fe0, some_data, 0x00000040));
  sdio1.VerifyAndClear();
}

// This test case verifies that whether an error will returned when transfer size is
// not divisible by 4.
TEST_F(SdioTest, AlignSize) {
  FakeSdioDevice device;
  std::unique_ptr<FakeSdioBus> sdio_bus;
  ASSERT_OK(FakeSdioBus::Create(&sdio_bus));
  brcmf_sdio_dev sdio_dev = {.drvr = device.drvr(), .bus = sdio_bus->get()};
  sdio_func func1 = {};
  pthread_mutex_init(&func1.lock, nullptr);

  MockSdio sdio1;

  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.func1 = &func1;

  sdio_bus->ExpectDoRwTxn(sdio1, ZX_OK, 0x00008000, 0x00000020, true, true);

  uint8_t some_data[128] = {};
  // 4-byte-aligned size should succeed.
  EXPECT_OK(brcmf_sdiod_ramrw(&sdio_dev, true, 0x00000000, some_data, 0x00000020));
  // non-4-byte-aligned size for sending should fail and return ZX_ERR_INVALID_ARGS.
  EXPECT_EQ(brcmf_sdiod_ramrw(&sdio_dev, true, 0x00000000, some_data, 0x00000021),
            ZX_ERR_INVALID_ARGS);
  // non-4-byte-aligned size for receiving should fail and return ZX_ERR_INVALID_ARGS.
  EXPECT_EQ(brcmf_sdiod_ramrw(&sdio_dev, false, 0x00000000, some_data, 0x00000021),
            ZX_ERR_INVALID_ARGS);
  sdio1.VerifyAndClear();
}

zx_status_t fake_brcmf_schedule_recovery_worker(struct brcmf_pub* drvr) {
  drvr->drvr_resetting.store(true);
  return ZX_OK;
}

// Verify sdio_timeout recovery trigger logic in brcmf_sdiod_transfer_vmos().
TEST_F(SdioTest, SdioTimeoutRecoveryVmos) {
  FakeSdioDevice device;
  std::unique_ptr<FakeSdioBus> sdio_bus;
  ASSERT_OK(FakeSdioBus::Create(&sdio_bus));
  brcmf_sdio_dev sdio_dev = {.drvr = device.drvr(), .bus = sdio_bus->get()};
  sdio_func func1 = {};
  bool recovery_triggered = false;
  // Create RecoveryTrigger with the fake recovery worker.
  auto recovery_start_callback = std::make_shared<std::function<zx_status_t()>>();
  *recovery_start_callback = std::bind(&fake_brcmf_schedule_recovery_worker, device.drvr());
  device.drvr()->recovery_trigger =
      std::make_unique<wlan::brcmfmac::RecoveryTrigger>(recovery_start_callback);

  pthread_mutex_init(&func1.lock, nullptr);

  MockSdio sdio1;

  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.func1 = &func1;

  // The expected frame sequence: 4 frames with ZX_ERR_TIMED_OUT, 1 frame with ZX_OK, 5 frames with
  // ZX_ERR_TIMED_OUT.
  for (size_t i = 0; i < 4; i++) {
    sdio_bus->ExpectDoRwTxn(sdio1, ZX_ERR_TIMED_OUT, i * 0x00001000, FakeSdioBus::kFrameSize, true,
                            true);
  }

  sdio_bus->ExpectDoRwTxn(sdio1, ZX_OK, 0x00000000, FakeSdioBus::kFrameSize, true, true);

  for (size_t i = 0; i < 5; i++) {
    sdio_bus->ExpectDoRwTxn(sdio1, ZX_ERR_TIMED_OUT, i * 0x00001000, FakeSdioBus::kFrameSize, true,
                            true);
  }

  uint8_t some_data[FakeSdioBus::kFrameSize] = {};

  // Make sure the drvr_setting bit is false before any of the frames sent.
  recovery_triggered = device.drvr()->drvr_resetting.load();
  EXPECT_FALSE(recovery_triggered);

  // Write four frames and with ZX_ERR_TIMED_OUT returned.
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00000000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00001000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00002000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00003000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);

  // Write one frame with ZX_OK returned. This frame resets the sdio_timeout recovery trigger
  // counter.
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00000000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_OK);

  // Write one more frame with ZX_ERR_TIMED_OUT returned.
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00000000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);

  // Although there are already 5 ZX_ERR_TIMED_OUTs returned, the last frame resets the counter, so
  // no recovery should be triggered here.
  recovery_triggered = device.drvr()->drvr_resetting.load();
  EXPECT_FALSE(recovery_triggered);

  // Write four more frames with ZX_ERR_TIMED_OUT returned.
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00001000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00002000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00003000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(brcmf_sdiod_write(&sdio_dev, &sdio_dev.sdio_proto_fn1, 0x00004000, some_data,
                              FakeSdioBus::kFrameSize, false),
            ZX_ERR_TIMED_OUT);

  // sdio_timeout recovery should be triggered here since there are five contiguous ZX_ERR_TIMED_OUT
  // returned.
  recovery_triggered = device.drvr()->drvr_resetting.load();
  EXPECT_TRUE(recovery_triggered);

  sdio1.VerifyAndClear();
}

// Verify sdio_timeout recovery trigger logic in brcmf_sdiod_transfer_vmo().
TEST_F(SdioTest, SdioTimeoutRecoveryVmo) {
  FakeSdioDevice device;
  std::unique_ptr<FakeSdioBus> sdio_bus;
  ASSERT_OK(FakeSdioBus::Create(&sdio_bus));
  brcmf_sdio_dev sdio_dev = {.drvr = device.drvr(), .bus = sdio_bus->get()};

  {
    std::lock_guard lock(sdio_dev.bus->rx_tx_data.tx_space);
    sdio_dev.bus->rx_tx_data.tx_frame = std::move(*sdio_dev.bus->rx_tx_data.tx_space.Acquire());
  }
  sdio_func func1 = {};
  bool recovery_triggered = false;
  // Create RecoveryTrigger with the fake recovery worker.
  auto recovery_start_callback = std::make_shared<std::function<zx_status_t()>>();
  *recovery_start_callback = std::bind(&fake_brcmf_schedule_recovery_worker, device.drvr());
  device.drvr()->recovery_trigger =
      std::make_unique<wlan::brcmfmac::RecoveryTrigger>(recovery_start_callback);

  pthread_mutex_init(&func1.lock, nullptr);

  MockSdio sdio1;

  sdio_dev.sdio_proto_fn1 = *sdio1.GetProto();
  sdio_dev.func1 = &func1;
  uint32_t some_data = 0x1234;
  uint32_t addr = 0x0000000;
  SBSDIO_FORMAT_ADDR(addr);

  // The expected frame sequence: 4 frames with ZX_ERR_TIMED_OUT, 1 frame with ZX_OK, 5 frames with
  // ZX_ERR_TIMED_OUT.
  for (size_t i = 0; i < 4; i++) {
    sdio_bus->ExpectDoRwTxn(sdio1, ZX_ERR_TIMED_OUT, addr, sizeof(some_data), true, true);
  }

  sdio_bus->ExpectDoRwTxn(sdio1, ZX_OK, addr, sizeof(some_data), true, true);

  for (size_t i = 0; i < 5; i++) {
    sdio_bus->ExpectDoRwTxn(sdio1, ZX_ERR_TIMED_OUT, addr, sizeof(some_data), true, true);
  }

  zx_status_t result = ZX_OK;
  // Make sure the drvr_setting bit is false before any of the frames sent.
  recovery_triggered = device.drvr()->drvr_resetting.load();
  EXPECT_FALSE(recovery_triggered);

  // Write four frames and with ZX_ERR_TIMED_OUT returned.
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);

  // Write one frame with ZX_OK returned. This frame resets the sdio_timeout recovery trigger
  // counter.
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_OK);

  // Write one more frame with ZX_ERR_TIMED_OUT returned.
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);

  // Although there are already 5 ZX_ERR_TIMED_OUTs returned, the last frame resets the counter, so
  // no recovery should be triggered here.
  recovery_triggered = device.drvr()->drvr_resetting.load();
  EXPECT_FALSE(recovery_triggered);

  // Write four more frames with ZX_ERR_TIMED_OUT returned.
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);
  brcmf_sdiod_func1_wl(&sdio_dev, 0x00000000, some_data, &result);
  EXPECT_EQ(result, ZX_ERR_TIMED_OUT);

  // sdio_timeout recovery should be triggered here since there are five contiguous ZX_ERR_TIMED_OUT
  // returned.
  recovery_triggered = device.drvr()->drvr_resetting.load();
  EXPECT_TRUE(recovery_triggered);

  sdio1.VerifyAndClear();
}

/*
 * A minimially initialized `struct brcmf_sdio` instance.
 *
 * This is contained in a class because `struct brcmf_sdio` holds pointers to various other structs.
 * To ensure memory is valid during usage, we package all of them together in this class.
 */
struct MinimalBrcmfSdio {
  MinimalBrcmfSdio(enum brcmf_sdiod_state sdiod_state, zx_duration_t ctl_done_timeout,
                   const char* workqueue_name, void (*work_item_handler)(WorkItem* work))
      : wq(workqueue_name) {
    sdio_dev = {.ctl_done_timeout = ctl_done_timeout, .drvr = &drvr, .state = sdiod_state};

    drvr.recovery_trigger = std::make_unique<wlan::brcmfmac::RecoveryTrigger>(nullptr);

    pthread_mutex_init(&func1.lock, nullptr);
    sdio_dev.func1 = &func1;

    bus_if.bus_priv.sdio = &sdio_dev;
    sdio_dev.bus_if = &bus_if;

    sdio_dev.bus = &bus;
    bus.sdiodev = &sdio_dev;

    bus.timer = &timer;

    // Prepare a WorkQueue with a single WorkItem to run the work_item_handler.
    bus.brcmf_wq = &wq;
    bus.datawork = WorkItem(work_item_handler);
    bus.dpc_triggered.store(false);
  }

  struct brcmf_pub drvr{};
  brcmf_sdio_dev sdio_dev{};
  sdio_func func1{};
  sdio_func func2{};
  struct brcmf_bus bus_if{};
  struct brcmf_sdio bus{};
  WorkQueue wq;

  // Fake timer that does nothing. This needs to exist for ResetClearsTxGlom test.
  Timer timer{fdf::Dispatcher::GetCurrent()->async_dispatcher(), [] {}, Timer::Type::OneShot};
};

/*
 * The sdio_bus_txctl_test() test helper calls brcmf_sdio_bus_txctl() by mocking the state around
 * the call using the function arguments. This helper returns the status returned by the
 * brcmf_sdio_bus_txctl() call.
 *
 *   sdiod_state       - state of the brcmf_sdio_dev contained in brcmf_bus
 *   ctl_done_timeout  - duration brcmf_sdio_bus_txctl() should wait for the
 *                       work_item_handler to complete before timing out
 *   workqueue_name    - name of the workqueue that effecively mocks dpc
 *   work_item_handler - WorkItem loaded into a WorkQueue to mock dpc
 *   expected_tx_ctlpkts - expected value of sdcnt.tx_ctlpkts after calling brcmf_sdio_bus_txctl()
 *   expected_tx_ctlerrs - expected value of sdcnt.tx_ctlerrs after calling brcmf_sdio_bus_txctl()
 *
 */
static zx_status_t sdio_bus_txctl_test(enum brcmf_sdiod_state sdiod_state,
                                       zx_duration_t ctl_done_timeout, const char* workqueue_name,
                                       void (*work_item_handler)(WorkItem* work),
                                       ulong expected_tx_ctlpkts, ulong expected_tx_ctlerrs) {
  MinimalBrcmfSdio b(sdiod_state, ctl_done_timeout, workqueue_name, work_item_handler);

  // Call brcmf_sdio_bus_txctl() with a blank message. Message processing is not mocked,
  // so this call purely tests
  unsigned char msg[] = "";
  const uint msglen = 0;
  zx_status_t status = brcmf_sdio_bus_txctl(&b.bus_if, msg, msglen);
  EXPECT_EQ(b.bus.sdcnt.tx_ctlpkts, expected_tx_ctlpkts);
  EXPECT_EQ(b.bus.sdcnt.tx_ctlerrs, expected_tx_ctlerrs);

  return status;
}

TEST_F(SdioTest, TxCtlSdioDown) {
  zx_status_t status = sdio_bus_txctl_test(
      BRCMF_SDIOD_DOWN, ZX_MSEC(CTL_DONE_TIMEOUT_MSEC), "brcmf_wq/txctl_sdio_down",
      [](WorkItem* work_item) {}, 0, 0);
  EXPECT_EQ(status, ZX_ERR_IO);
}

TEST_F(SdioTest, TxCtlOk) {
  zx_status_t status = sdio_bus_txctl_test(
      BRCMF_SDIOD_DATA, ZX_MSEC(CTL_DONE_TIMEOUT_MSEC), "brcmf_wq/txctl_ok",
      [](WorkItem* work) {
        struct brcmf_sdio* bus = containerof(work, struct brcmf_sdio, datawork);
        brcmf_sdio_if_ctrl_frame_stat_set(bus, [&bus]() {
          bus->ctrl_frame_err = ZX_OK;
          std::atomic_thread_fence(std::memory_order_seq_cst);
          brcmf_sdio_wait_event_wakeup(bus);
        });
      },
      1, 0);
  EXPECT_EQ(status, ZX_OK);
}

TEST_F(SdioTest, TxCtlTimeout) {
  zx_status_t status = sdio_bus_txctl_test(
      BRCMF_SDIOD_DATA, ZX_MSEC(1), "brcmf_wq/txctl_timeout", [](WorkItem* work) {}, 0, 1);
  EXPECT_EQ(status, ZX_ERR_TIMED_OUT);
}

TEST_F(SdioTest, TxCtlTimeoutUnexpectedCtrlFrameStatClear) {
  zx_status_t status = sdio_bus_txctl_test(
      BRCMF_SDIOD_DATA, ZX_MSEC(1), "brcmf_wq/txctl_timeout_unexpected_ctrl_frame_stat_clear",
      [](WorkItem* work) {
        struct brcmf_sdio* bus = containerof(work, struct brcmf_sdio, datawork);
        brcmf_sdio_if_ctrl_frame_stat_set(bus, [&bus]() { bus->ctrl_frame_err = ZX_OK; });
      },
      0, 1);
  EXPECT_EQ(status, ZX_ERR_TIMED_OUT);
}

TEST_F(SdioTest, TxCtlCtrlFrameStateNotCleared) {
  zx_status_t status = sdio_bus_txctl_test(
      BRCMF_SDIOD_DATA, ZX_MSEC(CTL_DONE_TIMEOUT_MSEC),
      "brcmf_wq/txctl_ctrl_frame_stat_not_cleared",
      [](WorkItem* work) {
        struct brcmf_sdio* bus = containerof(work, struct brcmf_sdio, datawork);
        std::atomic_thread_fence(std::memory_order_seq_cst);
        brcmf_sdio_wait_event_wakeup(bus);
      },
      0, 1);
  EXPECT_EQ(status, ZX_ERR_SHOULD_WAIT);
}

TEST_F(SdioTest, TxCtlCtrlFrameStateClearedWithError) {
  zx_status_t status = sdio_bus_txctl_test(
      BRCMF_SDIOD_DATA, ZX_MSEC(CTL_DONE_TIMEOUT_MSEC),
      "brcmf_wq/txctl_ctrl_frame_stat_cleared_with_error",
      [](WorkItem* work) {
        struct brcmf_sdio* bus = containerof(work, struct brcmf_sdio, datawork);
        brcmf_sdio_if_ctrl_frame_stat_set(bus, [&bus]() {
          bus->ctrl_frame_err = ZX_ERR_NO_MEMORY;
          std::atomic_thread_fence(std::memory_order_seq_cst);
          brcmf_sdio_wait_event_wakeup(bus);
        });
      },
      1, 0);
  EXPECT_EQ(status, ZX_ERR_NO_MEMORY);
}

TEST_F(SdioTest, ResetClearsTxGlom) {
  MinimalBrcmfSdio b(BRCMF_SDIOD_DATA, ZX_MSEC(1), "brcmf_wq/reset_clears_txglom",
                     [](WorkItem*) {});
  b.bus.txglom = true;
  brcmf_sdio_reset(&b.bus);
  ASSERT_FALSE(b.bus.txglom);
}
}  // namespace
