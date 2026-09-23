// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/env.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/zx/clock.h>

#include <gtest/gtest.h>
#include <src/storage/lib/block_protocol/block-fifo.h>

#include "../controller.h"
#include "fake-bus.h"
#include "src/lib/testing/predicates/status.h"

namespace ahci {
namespace {

class PortTest : public ::testing::Test {
 protected:
  void TearDown() override { fake_bus_.reset(); }

  void PortEnable(Bus* bus, Port* port) {
    uint32_t cap;
    EXPECT_OK(bus->RegRead(kHbaCapabilities, &cap));
    const uint32_t max_command_tag = (cap >> 8) & 0x1f;
    EXPECT_OK(port->Configure(0, bus, kHbaPorts, max_command_tag));
    EXPECT_OK(port->Enable());

    // Fake detect of device.
    port->set_device_present(true);

    EXPECT_TRUE(port->device_present());
    EXPECT_TRUE(port->port_implemented());
    EXPECT_TRUE(port->is_valid());
    EXPECT_FALSE(port->paused_cmd_issuing());
  }

  void BusAndPortEnable(Port* port) {
    std::unique_ptr<FakeBus> bus(new FakeBus());
    EXPECT_OK(bus->Configure());

    PortEnable(bus.get(), port);

    fake_bus_ = std::move(bus);
  }

  // If non-null, this pointer is owned by Controller::bus_
  std::unique_ptr<FakeBus> fake_bus_;

  fdf_testing::ScopedGlobalLogger logger_;
};

TEST(SataTest, SataStringFixTest) {
  // Nothing to do.
  SataStringFix(nullptr, 0);

  // Zero length, no swapping happens.
  uint16_t a = 0x1234;
  SataStringFix(&a, 0);
  ZX_ASSERT_MSG(a == 0x1234, "unexpected string result");

  // One character, only swap to even lengths.
  a = 0x1234;
  SataStringFix(&a, 1);
  ZX_ASSERT_MSG(a == 0x1234, "unexpected string result");

  // Swap A.
  a = 0x1234;
  SataStringFix(&a, sizeof(a));
  ZX_ASSERT_MSG(a == 0x3412, "unexpected string result");

  // Swap a group of values.
  uint16_t b[] = {0x0102, 0x0304, 0x0506};
  SataStringFix(b, sizeof(b));
  const uint16_t b_rev[] = {0x0201, 0x0403, 0x0605};
  ZX_ASSERT_MSG(memcmp(b, b_rev, sizeof(b)) == 0, "unexpected string result");

  // Swap a string.
  const char* qemu_model_id = "EQUMH RADDSI K";
  const char* qemu_rev = "QEMU HARDDISK ";
  const size_t qsize = strlen(qemu_model_id);

  union {
    uint16_t word[10];
    char byte[20];
  } str;

  memcpy(str.byte, qemu_model_id, qsize);
  SataStringFix(str.word, qsize);
  ZX_ASSERT_MSG(memcmp(str.byte, qemu_rev, qsize) == 0, "unexpected string result");

  const char* sin = "abcdefghijklmnoprstu";  // 20 chars
  const size_t slen = strlen(sin);
  ZX_ASSERT_MSG(slen == 20, "bad string length");
  ZX_ASSERT_MSG((slen & 1) == 0, "string length must be even");
  char sout[22];
  memset(sout, 0, sizeof(sout));
  memcpy(sout, sin, slen);

  // Verify swapping the length of every pair from 0 to 20 chars, inclusive.
  for (size_t i = 0; i <= slen; i += 2) {
    memcpy(str.byte, sin, slen);
    SataStringFix(str.word, i);
    ZX_ASSERT_MSG(memcmp(str.byte, sout, slen) == 0, "unexpected string result");
    ZX_ASSERT_MSG(sout[slen] == 0, "buffer overrun");
    char c = sout[i];
    sout[i] = sout[i + 1];
    sout[i + 1] = c;
  }
}

TEST_F(PortTest, PortTestEnable) {
  Port port;
  BusAndPortEnable(&port);
}

TEST_F(PortTest, PortCompleteNone) {
  Port port;
  BusAndPortEnable(&port);

  // Complete with no running transactions.

  EXPECT_FALSE(port.Complete());
}

TEST_F(PortTest, PortCompleteRunning) {
  Port port;
  BusAndPortEnable(&port);

  // Complete with running transaction. No completion should occur, cb_assert should not fire.

  SataTransaction txn = {};
  txn.timeout = zx::clock::get_monotonic() + zx::sec(5);
  txn.completion_cb = [](zx_status_t status) { EXPECT_TRUE(false); };

  uint32_t slot = 0;

  // Set txn as running.
  port.TestSetRunning(&txn, slot);
  // Set the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, (1u << slot));

  // Set interrupt for successful transfer completion, but keep the running bit set.
  // Simulates a non-error interrupt that will cause the IRQ handler to examin the running
  // transactions.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  EXPECT_TRUE(port.Complete());
}

TEST_F(PortTest, PortCompleteSuccess) {
  Port port;
  BusAndPortEnable(&port);

  // Transaction has successfully completed.

  zx_status_t completion_status = 100;
  SataTransaction txn = {};
  txn.timeout = zx::clock::get_monotonic() + zx::sec(5);
  txn.completion_cb = [&completion_status](zx_status_t status) { completion_status = status; };

  uint32_t slot = 0;

  // Set txn as running.
  port.TestSetRunning(&txn, slot);
  // Clear the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, 0);

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  // False means no more running commands.
  EXPECT_FALSE(port.Complete());
  // Set by completion callback.
  EXPECT_OK(completion_status);
}

TEST_F(PortTest, PortCompleteTimeout) {
  Port port;
  BusAndPortEnable(&port);

  // Transaction has successfully completed.

  zx_status_t completion_status = ZX_OK;
  SataTransaction txn = {};
  txn.timeout = zx::clock::get_monotonic() - zx::sec(1);
  txn.completion_cb = [&completion_status](zx_status_t status) { completion_status = status; };

  uint32_t slot = 0;

  // Set txn as running.
  port.TestSetRunning(&txn, slot);
  // Set the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, (1u << slot));

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  // False means no more running commands.
  EXPECT_FALSE(port.Complete());
  // Set by completion callback.
  EXPECT_NE(completion_status, ZX_OK);
}

TEST_F(PortTest, FlushWhenCommandQueueEmpty) {
  Port port;
  BusAndPortEnable(&port);

  SataDeviceInfo di;
  di.block_size = 512;
  di.max_cmd = 31;
  port.SetDevInfo(&di);

  zx_status_t status = ZX_ERR_IO;  // Value to be overwritten by callback.

  SataTransaction txn = {};
  txn.operation = block_server::Operation{
      .tag = block_server::Operation::Tag::Flush,
  };
  txn.completion_cb = [&status](zx_status_t st) { status = st; };
  txn.cmd = SATA_CMD_FLUSH_EXT;

  // Queue txn.
  port.Queue(&txn);  // Sets txn.timeout.

  // Process txn while the port has paused command issuing.
  EXPECT_TRUE(port.ProcessQueued());
  EXPECT_TRUE(port.paused_cmd_issuing());

  // Clear the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, 0);
  fake_bus_->PortRegOverride(0, kPortCommandIssue, 0);

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  // There are no more running commands (txn complete), and the port has unpaused.
  EXPECT_FALSE(port.Complete());
  EXPECT_FALSE(port.paused_cmd_issuing());
  EXPECT_EQ(status, ZX_OK);

  // There are no more commands to process.
  EXPECT_FALSE(port.ProcessQueued());
  EXPECT_FALSE(port.paused_cmd_issuing());
}

TEST_F(PortTest, FlushWhenWritePrecedingAndReadFollowing) {
  Port port;
  BusAndPortEnable(&port);

  SataDeviceInfo di;
  di.block_size = 512;
  di.max_cmd = 31;
  port.SetDevInfo(&di);

  zx_status_t write_status = ZX_ERR_IO;  // Value to be overwritten by callback.

  SataTransaction write_txn = {};
  write_txn.operation = block_server::Operation{
      .tag = block_server::Operation::Tag::Write,
  };
  write_txn.completion_cb = [&write_status](zx_status_t st) { write_status = st; };
  write_txn.cmd = SATA_CMD_WRITE_FPDMA_QUEUED;

  // Queue write_txn.
  port.Queue(&write_txn);

  zx_status_t flush_status = ZX_ERR_IO;  // Value to be overwritten by callback.

  SataTransaction flush_txn = {};
  flush_txn.operation = block_server::Operation{
      .tag = block_server::Operation::Tag::Flush,
  };
  flush_txn.completion_cb = [&flush_status](zx_status_t st) { flush_status = st; };
  flush_txn.cmd = SATA_CMD_FLUSH_EXT;

  // Queue flush_txn.
  port.Queue(&flush_txn);

  zx_status_t read_status = ZX_ERR_IO;  // Value to be overwritten by callback.

  SataTransaction read_txn = {};
  read_txn.operation = block_server::Operation{
      .tag = block_server::Operation::Tag::Read,
  };
  read_txn.completion_cb = [&read_status](zx_status_t st) { read_status = st; };
  read_txn.cmd = SATA_CMD_READ_FPDMA_QUEUED;

  // Queue read_txn.
  port.Queue(&read_txn);

  // Process write_txn while the port has paused command issuing.
  EXPECT_TRUE(port.ProcessQueued());
  EXPECT_TRUE(port.paused_cmd_issuing());

  // Clear the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, 0);
  fake_bus_->PortRegOverride(0, kPortCommandIssue, 0);

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  // There are no more running commands (write_txn complete), and the port has unpaused.
  EXPECT_FALSE(port.Complete());
  EXPECT_FALSE(port.paused_cmd_issuing());
  EXPECT_EQ(write_status, ZX_OK);

  // Process flush_txn while the port has paused command issuing.
  EXPECT_TRUE(port.ProcessQueued());
  EXPECT_TRUE(port.paused_cmd_issuing());

  // Clear the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, 0);
  fake_bus_->PortRegOverride(0, kPortCommandIssue, 0);

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  // There are no more running commands (flush_txn complete), and the port has unpaused.
  EXPECT_FALSE(port.Complete());
  EXPECT_FALSE(port.paused_cmd_issuing());
  EXPECT_EQ(flush_status, ZX_OK);

  // Process read_txn. The port remains unpaused.
  EXPECT_TRUE(port.ProcessQueued());
  EXPECT_FALSE(port.paused_cmd_issuing());

  // Clear the running bit in the bus.
  fake_bus_->PortRegOverride(0, kPortSataActive, 0);
  fake_bus_->PortRegOverride(0, kPortCommandIssue, 0);

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(0, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  port.HandleIrq();

  // There are no more running commands (read_txn complete).
  EXPECT_FALSE(port.Complete());
  EXPECT_FALSE(port.paused_cmd_issuing());
  EXPECT_EQ(read_status, ZX_OK);

  // There are no more commands to process.
  EXPECT_FALSE(port.ProcessQueued());
  EXPECT_FALSE(port.paused_cmd_issuing());
}

class TestController : public Controller {
 public:
  // Modify to configure the behaviour of this test controller.
  static bool support_native_command_queuing_;

  static constexpr uint32_t kTestLogicalBlockCount = 1024;

  explicit TestController() : Controller() {}

  zx::result<std::unique_ptr<Bus>> CreateBus() override {
    // Create a fake bus.
    auto fake_bus = std::make_unique<FakeBus>(support_native_command_queuing_);

    auto dispatcher = fdf::SynchronizedDispatcher::Create(
        fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "sata-background-init",
        [this](fdf_dispatcher_t*) { test_shutdown_completion_.Signal(); });
    if (dispatcher.is_error()) {
      return dispatcher.take_error();
    }
    test_dispatcher_ = *std::move(dispatcher);

    zx_status_t post_status = async::PostTask(
        test_dispatcher_.async_dispatcher(),
        fit::function<void()>([this, fake_bus = fake_bus.get()] {
          fdf::info("CreateBus background task started");
          Port* port = this->port(FakeBus::kTestPortNumber);
          const SataTransaction* command;
          while (true) {
            command = port->TestGetRunning(0);
            if (command != nullptr) {
              break;
            }
            // Wait until IDENTIFY DEVICE command is processed by the worker dispatcher thread.
            zx::nanosleep(zx::deadline_after(zx::msec(1)));
          }
          ASSERT_EQ(command->cmd, SATA_CMD_IDENTIFY_DEVICE);

          // Perform IDENTIFY DEVICE command.
          SataIdentifyDeviceResponse devinfo{};
          devinfo.major_version = 1 << 10;  // Support ACS-3.
          devinfo.capabilities_1 = 1 << 9;  // Spec simply says, "Shall be set to one."
          devinfo.lba_capacity = kTestLogicalBlockCount;
          zx::unowned_vmo vmo(command->vmo->get());
          ASSERT_OK(vmo->write(&devinfo, 0, sizeof(devinfo)));

          // Clear the running bit in the bus.
          fake_bus->PortRegOverride(FakeBus::kTestPortNumber, kPortSataActive, 0);
          fake_bus->PortRegOverride(FakeBus::kTestPortNumber, kPortCommandIssue, 0);

          // Set interrupt for successful transfer completion.
          fake_bus->PortRegOverride(FakeBus::kTestPortNumber, kPortInterruptStatus,
                                    AHCI_PORT_INT_DP);
          // Invoke interrupt handler.
          fake_bus->InterruptTrigger();
        }));
    ZX_ASSERT(post_status == ZX_OK);

    return zx::ok(std::move(fake_bus));
  }

  void Stop(fdf::StopCompleter completer) override {
    if (test_dispatcher_.get()) {
      test_dispatcher_.ShutdownAsync();
      test_shutdown_completion_.Wait();
    }
    Shutdown();

    if (sata_devices().empty()) {
      completer(zx::ok());
      return;
    }

    auto shared_completer = std::make_shared<fdf::StopCompleter>(std::move(completer));
    auto count = std::make_shared<std::atomic<size_t>>(sata_devices().size());

    for (auto& device : sata_devices()) {
      device->Shutdown([shared_completer, count]() {
        if (count->fetch_sub(1) == 1) {
          (*shared_completer)(zx::ok());
        }
      });
    }
  }

 private:
  fdf::Dispatcher test_dispatcher_;
  // Signaled when test_dispatcher_ is shut down.
  libsync::Completion test_shutdown_completion_;
};

bool TestController::support_native_command_queuing_;

class TestConfig final {
 public:
  using DriverType = TestController;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class AhciTest : public ::testing::TestWithParam<bool> {
 public:
  void SetUp() override {
    TestController::support_native_command_queuing_ = GetParam();
    driver_test().runtime().StartBackgroundDispatcher();

    ASSERT_OK(zx::event::create(0, &node_token_));
    zx::event token_copy;
    ASSERT_OK(node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

    zx::result<> result = driver_test().StartDriverWithCustomStartArgs(
        [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); });
    ASSERT_OK(result);

    driver_test().RunInDriverContext([this](TestController& driver) {
      fake_bus_ = static_cast<FakeBus*>(driver.bus());
      sata_device_ = driver.sata_devices()[0].get();
    });
    ASSERT_NE(fake_bus_, nullptr);
    ASSERT_NE(sata_device_, nullptr);
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_OK(result);
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 protected:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  FakeBus* fake_bus_;
  SataDevice* sata_device_;
  zx::event node_token_;
};

TEST_P(AhciTest, SataDeviceRead) {
  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server, this](TestController& driver) mutable {
    sata_device_->ServeRequests(std::move(volume_server));
  });

  auto [session_client, session_server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_OK(fidl::WireCall(volume_client)->OpenSession(std::move(session_server)).status());

  auto info_result = fidl::WireCall(volume_client)->GetInfo();
  ASSERT_OK(info_result.status());
  ASSERT_TRUE(info_result.value().is_ok());
  EXPECT_EQ(info_result.value().value()->info.block_size, 512u);
  EXPECT_EQ(info_result.value().value()->info.block_count, TestController::kTestLogicalBlockCount);
  if (TestController::support_native_command_queuing_) {
    EXPECT_TRUE(info_result.value().value()->info.flags &
                fuchsia_storage_block::wire::DeviceFlag::kFuaSupport);
  } else {
    EXPECT_FALSE(info_result.value().value()->info.flags &
                 fuchsia_storage_block::wire::DeviceFlag::kFuaSupport);
  }

  driver_test().RunInDriverContext([&](TestController& driver) {
    auto hierarchy = inspect::ReadFromVmo(driver.inspect().DuplicateVmo());
    ASSERT_TRUE(hierarchy.is_ok());
    const auto* ahci = hierarchy.value().GetByPath({"ahci"});
    ASSERT_NE(ahci, nullptr);
    const auto* ncq =
        ahci->node().get_property<inspect::BoolPropertyValue>("native_command_queuing");
    ASSERT_NE(ncq, nullptr);
    EXPECT_EQ(ncq->value(), TestController::support_native_command_queuing_);
  });

  auto fifo_result = fidl::WireCall(session_client)->GetFifo();
  ASSERT_OK(fifo_result.status());
  zx::fifo fifo = std::move(fifo_result.value()->fifo);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  zx::vmo dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

  auto vmo_result = fidl::WireCall(session_client)->AttachVmo(std::move(dup));
  ASSERT_OK(vmo_result.status());
  uint16_t vmoid = vmo_result.value()->vmoid.id;

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .reqid = 0,
      .group = 0,
      .vmoid = vmoid,
      .length = 1,
      .total_compressed_bytes = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(fifo.write(sizeof(BlockFifoRequest), &request, 1, nullptr));

  const SataTransaction* command = nullptr;
  while (true) {
    driver_test().RunInDriverContext([&command](TestController& driver) {
      Port* port = driver.port(FakeBus::kTestPortNumber);
      command = port->TestGetRunning(0);
    });
    if (command != nullptr) {
      break;
    }
    // Wait until read command is processed by the worker dispatcher thread.
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
  if (TestController::support_native_command_queuing_) {
    EXPECT_EQ(command->cmd, SATA_CMD_READ_FPDMA_QUEUED);
  } else {
    EXPECT_EQ(command->cmd, SATA_CMD_READ_DMA_EXT);
  }

  // Clear the running bit in the bus.
  fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortSataActive, 0);
  fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortCommandIssue, 0);

  // Set interrupt for successful transfer completion.
  fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortInterruptStatus, AHCI_PORT_INT_DP);
  // Invoke interrupt handler.
  fake_bus_->InterruptTrigger();
}

TEST_P(AhciTest, SataDeviceWriteWithFua) {
  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server, this](TestController& driver) mutable {
    sata_device_->ServeRequests(std::move(volume_server));
  });

  auto [session_client, session_server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_OK(fidl::WireCall(volume_client)->OpenSession(std::move(session_server)).status());

  auto fifo_result = fidl::WireCall(session_client)->GetFifo();
  ASSERT_OK(fifo_result.status());
  zx::fifo fifo = std::move(fifo_result.value()->fifo);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  zx::vmo dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

  auto vmo_result = fidl::WireCall(session_client)->AttachVmo(std::move(dup));
  ASSERT_OK(vmo_result.status());
  uint16_t vmoid = vmo_result.value()->vmoid.id;

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS},
      .reqid = 0,
      .group = 0,
      .vmoid = vmoid,
      .length = 1,
      .total_compressed_bytes = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(fifo.write(sizeof(BlockFifoRequest), &request, 1, nullptr));

  if (TestController::support_native_command_queuing_) {
    uint8_t cmd = 0;
    uint8_t device = 0;
    while (true) {
      driver_test().RunInDriverContext([&cmd, &device](TestController& driver) {
        Port* port = driver.port(FakeBus::kTestPortNumber);
        const SataTransaction* command = port->TestGetRunning(0);
        if (command != nullptr) {
          cmd = command->cmd;
          device = command->device;
        } else {
          cmd = 0;
        }
      });
      if (cmd != 0) {
        break;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    EXPECT_EQ(cmd, SATA_CMD_WRITE_FPDMA_QUEUED);
    EXPECT_EQ(device, 0xC0);

    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortSataActive, 0);
    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortCommandIssue, 0);
    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortInterruptStatus, AHCI_PORT_INT_DP);
    fake_bus_->InterruptTrigger();
  } else {
    // Without NCQ, the block server will simulate a FUA with a write + flush.
    uint8_t cmd = 0;
    uint8_t device = 0;
    while (true) {
      driver_test().RunInDriverContext([&cmd, &device](TestController& driver) {
        Port* port = driver.port(FakeBus::kTestPortNumber);
        const SataTransaction* command = port->TestGetRunning(0);
        if (command != nullptr) {
          cmd = command->cmd;
          device = command->device;
        } else {
          cmd = 0;
          device = 0;
        }
      });
      if (cmd == SATA_CMD_WRITE_DMA_EXT) {
        break;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    EXPECT_EQ(device, 0x40);

    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortSataActive, 0);
    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortCommandIssue, 0);
    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortInterruptStatus, AHCI_PORT_INT_DP);
    fake_bus_->InterruptTrigger();

    while (true) {
      driver_test().RunInDriverContext([&cmd](TestController& driver) {
        Port* port = driver.port(FakeBus::kTestPortNumber);
        const SataTransaction* command = port->TestGetRunning(0);
        if (command != nullptr) {
          cmd = command->cmd;
        } else {
          cmd = 0;
        }
      });
      if (cmd == SATA_CMD_FLUSH_EXT) {
        break;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }

    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortSataActive, 0);
    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortCommandIssue, 0);
    fake_bus_->PortRegOverride(FakeBus::kTestPortNumber, kPortInterruptStatus, AHCI_PORT_INT_DP);
    fake_bus_->InterruptTrigger();
  }

  ASSERT_OK(fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), nullptr));
  BlockFifoResponse response;
  size_t count = 0;
  ASSERT_OK(fifo.read(sizeof(BlockFifoResponse), &response, 1, &count));
  EXPECT_EQ(count, 1u);
  EXPECT_EQ(response.reqid, 0u);
  EXPECT_EQ(response.status, ZX_OK);
}

TEST_P(AhciTest, ShutdownWaitsForTransactionsInFlight) {
  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server, this](TestController& driver) mutable {
    sata_device_->ServeRequests(std::move(volume_server));
  });

  auto [session_client, session_server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_OK(fidl::WireCall(volume_client)->OpenSession(std::move(session_server)).status());

  auto fifo_result = fidl::WireCall(session_client)->GetFifo();
  ASSERT_OK(fifo_result.status());
  zx::fifo fifo = std::move(fifo_result.value()->fifo);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  zx::vmo dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

  auto vmo_result = fidl::WireCall(session_client)->AttachVmo(std::move(dup));
  ASSERT_OK(vmo_result.status());
  uint16_t vmoid = vmo_result.value()->vmoid.id;

  // Set up a transaction that will timeout (in 5 seconds by default).
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .reqid = 0,
      .group = 0,
      .vmoid = vmoid,
      .length = 1,
      .total_compressed_bytes = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(fifo.write(sizeof(BlockFifoRequest), &request, 1, nullptr));

  const SataTransaction* command = nullptr;
  while (true) {
    driver_test().RunInDriverContext([&command](TestController& driver) {
      Port* port = driver.port(FakeBus::kTestPortNumber);
      command = port->TestGetRunning(0);
    });
    if (command != nullptr) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
  ASSERT_NE(command, nullptr);

  zx::time time = zx::clock::get_monotonic();
  libsync::Completion shutdown_complete;
  driver_test().RunInDriverContext([&shutdown_complete, this](TestController& driver) {
    sata_device_->Shutdown([&shutdown_complete]() { shutdown_complete.Signal(); });
  });
  shutdown_complete.Wait();
  zx::duration shutdown_duration = zx::clock::get_monotonic() - time;

  // The shutdown duration should be around 5 seconds (+/-). Conservatively check for > 2.5 seconds.
  EXPECT_GT(shutdown_duration, Port::kTransactionTimeout / 2);

  // Verify that the client gets a response from the FIFO.
  BlockFifoResponse response;
  size_t count = 0;
  ASSERT_OK(fifo.read(sizeof(BlockFifoResponse), &response, 1, &count));
  EXPECT_EQ(count, 1u);
  EXPECT_EQ(response.reqid, 0u);
  EXPECT_EQ(response.status, ZX_ERR_CANCELED);
}

TEST_P(AhciTest, ShutdownWithHungDeviceClearsPortTransactions) {
  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server, this](TestController& driver) mutable {
    sata_device_->ServeRequests(std::move(volume_server));
  });

  auto [session_client, session_server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_OK(fidl::WireCall(volume_client)->OpenSession(std::move(session_server)).status());

  auto fifo_result = fidl::WireCall(session_client)->GetFifo();
  ASSERT_OK(fifo_result.status());
  zx::fifo fifo = std::move(fifo_result.value()->fifo);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  zx::vmo dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

  auto vmo_result = fidl::WireCall(session_client)->AttachVmo(std::move(dup));
  ASSERT_OK(vmo_result.status());
  uint16_t vmoid = vmo_result.value()->vmoid.id;

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .reqid = 0,
      .group = 0,
      .vmoid = vmoid,
      .length = 1,
      .total_compressed_bytes = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(fifo.write(sizeof(BlockFifoRequest), &request, 1, nullptr));

  SataTransaction* command = nullptr;
  while (true) {
    driver_test().RunInDriverContext([&command](TestController& driver) {
      Port* port = driver.port(FakeBus::kTestPortNumber);
      command = port->TestGetRunning(0);
      if (command != nullptr) {
        command->timeout = zx::time::infinite();
      }
    });
    if (command != nullptr) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
  ASSERT_NE(command, nullptr);

  libsync::Completion shutdown_complete;
  driver_test().RunInDriverContext([&shutdown_complete, this](TestController& driver) {
    sata_device_->Shutdown([&shutdown_complete]() { shutdown_complete.Signal(); });
  });

  shutdown_complete.Wait();

  driver_test().RunInDriverContext([](TestController& driver) { driver.sata_devices().clear(); });

  driver_test().RunInDriverContext([](TestController& driver) {
    Port* port = driver.port(FakeBus::kTestPortNumber);
    port->Complete();
  });
}

TEST_P(AhciTest, NodeToken) {
  zx::result connect_result =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Token>("sata0");
  ASSERT_OK(connect_result);

  fidl::SyncClient<fuchsia_driver_token::NodeToken> client(std::move(connect_result.value()));
  auto get_result = client->Get();
  ASSERT_TRUE(get_result.is_ok());

  zx_info_handle_basic_t info1, info2;
  ASSERT_EQ(node_token_.get_info(ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1), nullptr, nullptr),
            ZX_OK);
  ASSERT_EQ(
      get_result->token().get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr),
      ZX_OK);
  ASSERT_EQ(info1.koid, info2.koid);
}

INSTANTIATE_TEST_SUITE_P(NativeCommandQueuingSupportTest, AhciTest, ::testing::Bool());

}  // namespace
}  // namespace ahci

FUCHSIA_DRIVER_EXPORT2(ahci::TestController);
