// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/nvme/nvme.h"

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/fake-bti/bti.h>
#include <lib/fdf/env.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <atomic>
#include <memory>
#include <thread>

#include <gtest/gtest.h>

#include "src/devices/block/drivers/nvme/commands/nvme-io.h"
#include "src/devices/block/drivers/nvme/fake/fake-admin-commands.h"
#include "src/devices/block/drivers/nvme/fake/fake-controller.h"
#include "src/devices/block/drivers/nvme/fake/fake-namespace.h"
#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/buffer/vmoid_registry.h"

namespace nvme {

class TestNvme : public Nvme {
 public:
  // Modify to configure the behaviour of this test driver.
  static fake_nvme::FakeController controller_;

  TestNvme() = default;

 protected:
  zx::result<fit::function<void()>> InitResources() override {
    pci_ = ddk::Pci{};
    mmio_ = controller_.registers().GetBuffer();

    // Create a fake BTI.
    zx::bti fake_bti;
    ZX_ASSERT(fake_bti_create(fake_bti.reset_and_get_address()) == ZX_OK);
    bti_ = std::move(fake_bti);

    // Set up an interrupt.
    irq_mode_ = fuchsia_hardware_pci::InterruptMode::kMsiX;
    auto irq = controller_.GetOrCreateInterrupt(0);
    ZX_ASSERT(irq.is_ok());
    irq_ = std::move(*irq);

    controller_.SetNvme(this);
    return zx::ok([] {});
  }

  fake_nvme::FakeAdminCommands admin_commands_{controller_};
};

fake_nvme::FakeController TestNvme::controller_;

class TestConfig final {
 public:
  using DriverType = TestNvme;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class NvmeTest : public ::testing::Test {
 public:
  void StartDriver() {
    ASSERT_OK(zx::event::create(0, &node_token_));
    zx::event token_copy;
    ASSERT_OK(node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

    zx::result<> result = driver_test().StartDriverWithCustomStartArgs(
        [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); });
    ASSERT_OK(result);
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_OK(result);
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  void CheckStringPropertyPrefix(const inspect::NodeValue& node, const std::string& property,
                                 const char* expected) {
    const auto* actual = node.get_property<inspect::StringPropertyValue>(property);
    EXPECT_TRUE(actual);
    if (!actual) {
      return;
    }
    EXPECT_EQ(0, strncmp(actual->value().data(), expected, strlen(expected)));
  }

  void CheckBooleanProperty(const inspect::NodeValue& node, const std::string& property,
                            bool expected) {
    const auto* actual = node.get_property<inspect::BoolPropertyValue>(property);
    EXPECT_TRUE(actual);
    if (!actual) {
      return;
    }
    EXPECT_EQ(actual->value(), expected);
  }

 protected:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  zx::event node_token_;
};

class NvmeTeardownTest : public ::testing::Test {
 public:
  void StartDriver() {
    ASSERT_OK(zx::event::create(0, &node_token_));
    zx::event token_copy;
    ASSERT_OK(node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

    zx::result<> result = driver_test().StartDriverWithCustomStartArgs(
        [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); });
    ASSERT_OK(result);
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 protected:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  zx::event node_token_;
};

TEST_F(NvmeTest, BasicTest) {
  ASSERT_NO_FATAL_FAILURE(StartDriver());
  inspect::Inspector inspector = driver_test().RunInDriverContext<inspect::Inspector>(
      [](TestNvme& driver) { return driver.inspect(); });
  fpromise::result<inspect::Hierarchy> hierarchy =
      fpromise::run_single_threaded(inspect::ReadFromInspector(inspector));
  ASSERT_TRUE(hierarchy.is_ok());
  const auto* nvme = hierarchy.value().GetByPath({"nvme"});
  ASSERT_NE(nullptr, nvme);
  const auto* controller = nvme->GetByPath({"controller"});
  ASSERT_NE(nullptr, controller);
  CheckStringPropertyPrefix(controller->node(), "model_number",
                            fake_nvme::FakeAdminCommands::kModelNumber);
  CheckStringPropertyPrefix(controller->node(), "serial_number",
                            fake_nvme::FakeAdminCommands::kSerialNumber);
  CheckStringPropertyPrefix(controller->node(), "firmware_rev",
                            fake_nvme::FakeAdminCommands::kFirmwareRev);
  CheckBooleanProperty(controller->node(), "volatile_write_cache_enabled", true);
}

TEST_F(NvmeTest, NamespaceBlockInfo) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client =
      block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  client.value()->BlockGetInfo(&info);
  EXPECT_EQ(512u, info.block_size);
  EXPECT_EQ(1024u, info.block_count);
  EXPECT_TRUE(info.flags & fuchsia_storage_block::wire::DeviceFlag::kFuaSupport);
  client.value().reset();
}

TEST_F(NvmeTest, NamespaceReadTest) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);
  TestNvme::controller_.AddIoCommand(
      IoCommandOpcode::kRead,
      [](Submission& submission, const TransactionData& data, Completion& completion) {
        completion.set_status_code_type(StatusCodeType::kGeneric)
            .set_status_code(GenericStatus::kSuccess);
      });
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client =
      block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ::storage::Vmoid vmoid;
  ASSERT_OK(client.value()->BlockAttachVmo(vmo, &vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .vmoid = vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(client.value()->FifoTransaction(&request, 1));
  ASSERT_OK(client.value()->BlockDetachVmo(std::move(vmoid)));
  client.value().reset();
}

TEST_F(NvmeTest, DuplicateIoCompletion) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);
  TestNvme::controller_.AddIoCommand(
      IoCommandOpcode::kRead,
      [](Submission& submission, const TransactionData& data, Completion& completion) {
        completion.set_status_code_type(StatusCodeType::kGeneric)
            .set_status_code(GenericStatus::kSuccess);
        // Inject a duplicate completion into the CQ right after this one.
        Completion duplicate_completion = completion;
        TestNvme::controller_.SubmitCompletion(duplicate_completion);
      });
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client =
      block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ::storage::Vmoid vmoid;
  ASSERT_OK(client.value()->BlockAttachVmo(vmo, &vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .vmoid = vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(client.value()->FifoTransaction(&request, 1));
  ASSERT_OK(client.value()->BlockDetachVmo(std::move(vmoid)));
  client.value().reset();
}

TEST_F(NvmeTest, NamespaceWriteTest) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);
  TestNvme::controller_.AddIoCommand(
      IoCommandOpcode::kWrite,
      [](Submission& submission, const TransactionData& data, Completion& completion) {
        completion.set_status_code_type(StatusCodeType::kGeneric)
            .set_status_code(GenericStatus::kSuccess);
      });
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client =
      block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ::storage::Vmoid vmoid;
  ASSERT_OK(client.value()->BlockAttachVmo(vmo, &vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
      .vmoid = vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(client.value()->FifoTransaction(&request, 1));
  ASSERT_OK(client.value()->BlockDetachVmo(std::move(vmoid)));
  client.value().reset();
}

// TODO(https://fxbug.dev/510806838): Re-enable this test once nvme properly handles submitted
// requests during teardown.
TEST_F(NvmeTeardownTest, DISABLED_TeardownWithActiveClient) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);
  TestNvme::controller_.AddIoCommand(
      IoCommandOpcode::kRead,
      [](Submission& submission, const TransactionData& data, Completion& completion) {
        completion.set_status_code_type(StatusCodeType::kGeneric)
            .set_status_code(GenericStatus::kSuccess);
      });
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> device =
      block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(device);

  block_client::ReaderWriter client(*device.value());

  std::atomic<bool> stopped = false;
  std::thread t([&]() {
    while (!stopped) {
      uint8_t buffer[512];
      [[maybe_unused]] zx_status_t status = client.Read(0, sizeof(buffer), buffer);
    }
  });

  EXPECT_OK(driver_test().StopDriver());
  stopped = true;
  t.join();
}

TEST_F(NvmeTeardownTest, TeardownWithPendingRequest) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);

  libsync::Completion unblock;
  libsync::Completion in_flight;
  // Start a command which blocks, so the driver cannot issue any other commands.
  TestNvme::controller_.AddIoCommand(
      IoCommandOpcode::kRead,
      [&](Submission& submission, const TransactionData& data, Completion& completion) {
        in_flight.Signal();
        unblock.Wait();
        completion.set_status_code_type(StatusCodeType::kGeneric)
            .set_status_code(GenericStatus::kSuccess);
      });
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> device =
      block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(device);

  block_client::ReaderWriter client(*device.value());

  std::thread t([&]() {
    uint8_t buffer[512];
    [[maybe_unused]] zx_status_t status = client.Read(0, sizeof(buffer), buffer);
  });

  // Wait for the blocking command to reach the hardware.
  in_flight.Wait();

  std::thread unblock_thread([&]() {
    // Give StopDriver() time to reach ns->StopBlockServer() and block_server_->DestroyAsync().
    zx::nanosleep(zx::deadline_after(zx::msec(50)));
    unblock.Signal();
  });

  EXPECT_OK(driver_test().StopDriver());

  unblock_thread.join();
  t.join();
}

TEST_F(NvmeTest, IoPushback) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);

  // Block I/O so we can queue up a bunch of requests.
  libsync::Completion unblock;
  TestNvme::controller_.AddIoCommand(
      IoCommandOpcode::kRead,
      [&](Submission&, const TransactionData&, Completion&) { unblock.Wait(); });

  driver_test().runtime().StartBackgroundDispatcher();
  ASSERT_NO_FATAL_FAILURE(StartDriver());

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&volume_server](TestNvme& driver) mutable {
    Namespace* ns = driver.namespaces()[0].get();
    ns->ServeRequests(std::move(volume_server));
  });

  auto [session_client, session_server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  fidl::OneWayStatus result = fidl::WireCall(volume_client)->OpenSession(std::move(session_server));
  ASSERT_OK(result.status());

  fidl::WireResult fifo = fidl::WireCall(session_client)->GetFifo();
  ASSERT_OK(fifo.status());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  fidl::WireResult attach_result = fidl::WireCall(session_client)->AttachVmo(std::move(vmo));
  ASSERT_OK(attach_result.status());
  vmoid_t vmoid = attach_result.value()->vmoid.id;
  [[maybe_unused]] auto cleanup = fit::defer([&] {
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0},
        .vmoid = vmoid,
    };
    size_t actual_count = 0;
    ASSERT_OK(fifo->value()->fifo.write(sizeof(request), &request, 1, &actual_count));
    ASSERT_EQ(actual_count, 1u);
    ASSERT_OK(fifo->value()->fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), nullptr));
    BlockFifoResponse response;
    ASSERT_OK(fifo->value()->fifo.read(sizeof(response), &response, 1, &actual_count));
    ASSERT_EQ(actual_count, 1u);
    ASSERT_OK(response.status);
  });

  // Eventually the client should get pushback via ZX_ERR_SHOULD_WAIT.
  size_t num_requests = 0;
  while (true) {
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .vmoid = vmoid,
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    size_t actual_count = 0;
    zx_status_t status = fifo->value()->fifo.write(sizeof(request), &request, 1, &actual_count);
    if (status == ZX_ERR_SHOULD_WAIT) {
      break;
    }
    ASSERT_OK(status);
    ASSERT_EQ(actual_count, 1u);
    ++num_requests;
  }
  ASSERT_GT(num_requests, 0u);

  // Unblock I/O, and then we should eventually be able to read one response and write another
  // request to the FIFO.
  unblock.Signal();

  BlockFifoResponse response;
  ASSERT_OK(fifo.value()->fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), nullptr));
  size_t actual_count = 0;
  ASSERT_OK(fifo->value()->fifo.read(sizeof(response), &response, 1, &actual_count));
  ASSERT_EQ(actual_count, 1u);
  ASSERT_OK(response.status);
  --num_requests;

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .vmoid = vmoid,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  ASSERT_OK(fifo->value()->fifo.wait_one(ZX_FIFO_WRITABLE, zx::time::infinite(), nullptr));
  ASSERT_OK(fifo->value()->fifo.write(sizeof(request), &request, 1, &actual_count));
  ASSERT_EQ(actual_count, 1u);

  // Drain the rest of the requests and ensure that nothing failed.
  while (num_requests > 0) {
    ASSERT_OK(fifo->value()->fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(fifo->value()->fifo.read(sizeof(response), &response, 1, &actual_count));
    ASSERT_EQ(actual_count, 1u);
    ASSERT_OK(response.status);
    --num_requests;
  }

  ASSERT_OK(fifo->value()->fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), nullptr));
  ASSERT_OK(fifo->value()->fifo.read(sizeof(response), &response, 1, &actual_count));
  ASSERT_EQ(actual_count, 1u);
  ASSERT_OK(response.status);
}

TEST_F(NvmeTest, NodeToken) {
  fake_nvme::FakeNamespace fake_ns;
  TestNvme::controller_.AddNamespace(1, fake_ns);
  driver_test().runtime().StartBackgroundDispatcher();

  ASSERT_NO_FATAL_FAILURE(StartDriver());

  zx::result connect_result =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Token>("namespace-1");
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

TEST_F(NvmeTest, InitFailureCleansUpThreads) {
  TestNvme::controller_.AddAdminCommand(
      nvme::AdminCommandOpcode::kIdentify,
      [](Submission& default_submission, const TransactionData& data, Completion& completion) {
        completion.set_status_code_type(StatusCodeType::kGeneric)
            .set_status_code(GenericStatus::kInvalidField);
      });

  ASSERT_OK(zx::event::create(0, &node_token_));
  zx::event token_copy;
  ASSERT_OK(node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

  zx::result<> result = driver_test().StartDriverWithCustomStartArgs(
      [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); });
  EXPECT_TRUE(result.is_error());
}

}  // namespace nvme

FUCHSIA_DRIVER_EXPORT2(nvme::TestNvme);
