/*
 * Copyright (c) 2022 The Fuchsia Authors
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

#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "src/connectivity/ethernet/drivers/third_party/igc/igc_driver.h"
#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/lib/testing/predicates/status.h"

namespace {

namespace ei = ethernet::igc;
namespace netdev = fuchsia_hardware_network;
namespace netdriver = fuchsia_hardware_network_driver;

constexpr size_t kFakeBarSize = 0x20000;
constexpr uint32_t kTestDeviceId = 0x15F2;
constexpr uint32_t kTestSubsysId = 0x0001;
constexpr uint32_t kTestSubsysVendorId = 0x0abe;
constexpr uint32_t kTestCommand = 0x0033;
constexpr uint8_t kVmoId = 0;

class NetworkDeviceIfc : public fdf::WireServer<netdriver::NetworkDeviceIfc> {
 public:
  explicit NetworkDeviceIfc(fdf_dispatcher_t* dispatcher) {
    auto endpoints = fdf::CreateEndpoints<netdriver::NetworkDeviceIfc>();
    EXPECT_OK(endpoints.status_value());

    fdf::BindServer(dispatcher, std::move(endpoints->server), this);
    ifc_client_end_ = std::move(endpoints->client);
  }

  fdf::ClientEnd<netdriver::NetworkDeviceIfc> TakeIfcClientEnd() {
    return std::move(ifc_client_end_);
  }
  fdf::ClientEnd<netdriver::NetworkPort> TakePortClientEnd() { return std::move(port_client_end_); }

  void WaitUntilCompleteRxCalled() { complete_rx_called_.Wait(); }
  void WaitUntilCompleteTxCalled() { complete_tx_called_.Wait(); }

  const auto& TxResults() const { return tx_results_; }
  const auto& RxBuffers() const { return rx_buffers_; }
  const auto& RxBufferParts() const { return rx_buffer_parts_; }

  // NetworkDeviceIfc implementation
  void PortStatusChanged(netdriver::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                         fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) override {}
  void AddPort(netdriver::wire::NetworkDeviceIfcAddPortRequest* request, fdf::Arena& arena,
               AddPortCompleter::Sync& completer) override {
    port_client_end_ = std::move(request->port);
    EXPECT_EQ(request->id, ei::kPortId);
    completer.buffer(arena).Reply(ZX_OK);
  }
  void RemovePort(netdriver::wire::NetworkDeviceIfcRemovePortRequest* request, fdf::Arena& arena,
                  RemovePortCompleter::Sync& completer) override {}
  void CompleteRx(netdriver::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                  CompleteRxCompleter::Sync& completer) override {
    // Deep copy rx buffers.
    for (const auto& rx : request->rx) {
      rx_buffer_parts_.push_back(fidl::ToNatural(rx.data).value());
      rx_buffers_.push_back(fidl::ToNatural(rx));
    }
    complete_rx_called_.Signal();
  }
  void CompleteTx(netdriver::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                  CompleteTxCompleter::Sync& completer) override {
    // Deep copy tx_list to local record structure since the pointer will point to void after this
    // function returns.
    for (const auto& tx : request->tx) {
      tx_results_.push_back(fidl::ToNatural(tx));
    }
    complete_tx_called_.Signal();
  }
  void DelegateRxLease(netdriver::wire::NetworkDeviceIfcDelegateRxLeaseRequest* request,
                       fdf::Arena& arena, DelegateRxLeaseCompleter::Sync& completer) override {}
  void UpdateRxBufferParams(netdriver::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest* request,
                            fdf::Arena& arena,
                            UpdateRxBufferParamsCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(fit::ok());
  }
  void RequestRxSpace(netdriver::wire::NetworkDeviceIfcRequestRxSpaceRequest* request,
                      fdf::Arena& arena, RequestRxSpaceCompleter::Sync& completer) override {}

 private:
  fdf::ClientEnd<netdriver::NetworkDeviceIfc> ifc_client_end_;
  fdf::ClientEnd<netdriver::NetworkPort> port_client_end_;

  libsync::Completion complete_tx_called_;
  libsync::Completion complete_rx_called_;

  // The vector to record tx results for each packets from CompleteTx.
  std::vector<netdriver::TxResult> tx_results_;

  // The vector to record rx buffers received from CompleteRx.
  std::vector<netdriver::RxBuffer> rx_buffers_;

  // The local copy of buffer parts received from CompleteRx.
  std::vector<std::vector<netdriver::RxBufferPart>> rx_buffer_parts_;
};

// Create a minimally viable TX buffer with the required fields to pass the FIDL encoder.
template <typename IdType>
netdriver::wire::TxBuffer CreateTxBuffer(IdType buffer_id) {
  return netdriver::wire::TxBuffer{
      .id = static_cast<uint32_t>(buffer_id),
      .meta{
          .frame_type = netdev::wire::FrameType::kEthernet,
      },
  };
}

class TestFixtureEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing) override {
    device_server_.Initialize(component::kDefaultInstance);
    zx_status_t status =
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &outgoing);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    // Set up the first BAR.
    fake_pci_.CreateBar(/*bar_id=*/0, /*size=*/kFakeBarSize, /*is_mmio=*/true);

    // Identify as the correct device.
    fake_pci_.SetDeviceInfo({.device_id = kTestDeviceId});

    // Setup the configuration for fake PCI.
    zx::unowned_vmo config = fake_pci_.GetConfigVmo();
    config->write(&kTestSubsysId, fidl::ToUnderlying(fuchsia_hardware_pci::Config::kSubsystemId),
                  sizeof(kTestSubsysId));
    config->write(&kTestSubsysVendorId,
                  fidl::ToUnderlying(fuchsia_hardware_pci::Config::kSubsystemVendorId),
                  sizeof(kTestSubsysVendorId));
    config->write(&kTestCommand, fidl::ToUnderlying(fuchsia_hardware_pci::Config::kCommand),
                  sizeof(kTestCommand));

    fake_pci_.AddLegacyInterrupt();

    auto result = outgoing.AddService<fuchsia_hardware_pci::Service>(
        fuchsia_hardware_pci::Service::InstanceHandler(
            {.device = fake_pci_.bind_handler(fdf::Dispatcher::GetCurrent()->async_dispatcher())}));
    return result;
  }

  pci::FakePciProtocol fake_pci_;
  compat::DeviceServer device_server_;
};

struct TestFixtureConfig {
  using DriverType = ei::IgcDriver;
  using EnvironmentType = TestFixtureEnvironment;
};

class IgcInterfaceTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> start_result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, start_result.status_value());
    zx::result netdev_impl = driver_test().Connect<netdriver::Service::NetworkDeviceImpl>();
    ASSERT_OK(netdev_impl.status_value());

    netdev_client_.Bind(std::move(netdev_impl.value()),
                        driver_test().runtime().StartBackgroundDispatcher()->get());

    // AddPort() will be called inside this function and will also be verified.
    auto result = netdev_client_.sync().buffer(arena_)->Init(netdev_ifc_.TakeIfcClientEnd());
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);

    port_client_.Bind(netdev_ifc_.TakePortClientEnd(), fdf::Dispatcher::GetCurrent()->get());
    ASSERT_TRUE(port_client_.is_valid());

    // Create the VMO like what netdev driver does.
    zx::vmo::create(ei::kEthRxBufCount * ei::kEthRxBufSize + ei::kEthTxBufCount * ei::kEthTxBufSize,
                    kVmoId, &test_vmo_);

    // Override the write32 operator in the MMIO buffer so that we can trigger on certain calls.
    // These calls will write to a specific offset in the MMIO buffer on completion.
    override_ops_.Write32 = [](const void* ctx, const mmio_buffer_t& mmio, uint32_t val,
                               zx_off_t offs) {
      // Double casting because ctx is const, but we know it's safe to use as non-const here.
      auto test = const_cast<IgcInterfaceTest*>(static_cast<const IgcInterfaceTest*>(ctx));
      //  Forward the call to the actual write operation.
      fdf::internal::kDefaultOps.Write32(ctx, mmio, val, offs);
      if (offs == static_cast<zx_off_t>(IGC_TDT(0))) {
        // This is the offset used for QueueTx.
        std::lock_guard lock(test->tx_tail_mutex_);
        ++test->num_tx_tail_updates_;
        test->tx_tail_updated_.notify_all();
      } else if (offs == static_cast<zx_off_t>(IGC_RDT(0))) {
        // This is the offset used for QueueRxSpace.
        std::lock_guard lock(test->rx_tail_mutex_);
        ++test->num_rx_tail_updates_;
        test->rx_tail_updated_.notify_all();
      }
    };

    // Install the new operations in the MMIO buffer.
    driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
      auto adapter = driver.Adapter();
      // Release and re-use the same mmio_buffer, the only thing that should change is the ops.
      mmio_buffer_t buffer = adapter->osdep.mmio_buffer->release();
      adapter->osdep.mmio_buffer = fdf::MmioBuffer(buffer, &override_ops_, this);
    });
  }

  void TearDown() override { ASSERT_OK(driver_test().StopDriver().status_value()); }

  void WaitForTxTailUpdates(size_t num_updates_to_wait_for) {
    std::unique_lock lock(tx_tail_mutex_);
    while (num_tx_tail_updates_ != num_updates_to_wait_for) {
      tx_tail_updated_.wait(lock);
    }
    num_tx_tail_updates_ = 0;
  }

  void WaitForRxTailUpdates(size_t num_updates_to_wait_for) {
    std::unique_lock lock(rx_tail_mutex_);
    while (num_rx_tail_updates_ != num_updates_to_wait_for) {
      rx_tail_updated_.wait(lock);
    }
    num_rx_tail_updates_ = 0;
  }

  void ClearNumTxTailUpdates() {
    std::lock_guard lock(tx_tail_mutex_);
    num_tx_tail_updates_ = 0;
  }

  void ClearNumRxTailUpdates() {
    std::lock_guard lock(rx_tail_mutex_);
    num_rx_tail_updates_ = 0;
  }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig>& driver_test() { return driver_test_; }
  fdf_testing::BackgroundDriverTest<TestFixtureConfig> driver_test_;

  fdf::Arena arena_{'IGCT'};

  NetworkDeviceIfc netdev_ifc_{driver_test().runtime().StartBackgroundDispatcher()->get()};
  fdf::WireSharedClient<netdriver::NetworkDeviceImpl> netdev_client_;
  fdf::WireSharedClient<netdriver::NetworkPort> port_client_;

  // Used for MMIO buffer overrides and event triggering.
  fdf::MmioBufferOps override_ops_ = fdf::internal::kDefaultOps;

  std::condition_variable tx_tail_updated_;
  size_t num_tx_tail_updates_ = 0;
  std::mutex tx_tail_mutex_;

  std::condition_variable rx_tail_updated_;
  size_t num_rx_tail_updates_ = 0;
  std::mutex rx_tail_mutex_;

  // The VMO faked by this test class to simulate the one that netdevice creates in real case.
  zx::vmo test_vmo_;
};

TEST_F(IgcInterfaceTest, NetworkDeviceImplGetInfo) {
  auto result = netdev_client_.sync().buffer(arena_)->GetInfo();
  ASSERT_OK(result.status());

  netdriver::wire::DeviceImplInfo& info = result.value().info;

  EXPECT_EQ(info.tx_depth(), ei::kEthTxBufCount);
  EXPECT_EQ(info.rx_depth(), ei::kEthRxBufCount);
  EXPECT_EQ(info.rx_threshold(), info.rx_depth() / 2);
  EXPECT_EQ(info.max_buffer_parts(), 1U);
  EXPECT_EQ(info.max_buffer_length(), ei::kEthRxTxMaxBufSize);
  EXPECT_EQ(info.buffer_alignment(), ei::kEthRxTxBufAlignment);
  EXPECT_EQ(info.min_rx_buffer_length(), 2048U);
  EXPECT_EQ(info.min_tx_buffer_length(), 60U);
  EXPECT_EQ(info.tx_head_length(), 0U);
  EXPECT_EQ(info.tx_tail_length(), 0U);
}

TEST_F(IgcInterfaceTest, NetworkDeviceImplPrepareVmo) {
  libsync::Completion prepared;
  netdev_client_.buffer(arena_)
      ->PrepareVmo(0, std::move(test_vmo_))
      .Then([&](fdf::WireUnownedResult<netdriver::NetworkDeviceImpl::PrepareVmo>& result) {
        EXPECT_OK(result.status());
        EXPECT_OK(result.value().s);
        prepared.Signal();
      });
  prepared.Wait();
}

TEST_F(IgcInterfaceTest, NetworkDeviceImplStart) {
  libsync::Completion started;
  netdev_client_.buffer(arena_)->Start().Then(
      [&](fdf::WireUnownedResult<netdriver::NetworkDeviceImpl::Start>& result) {
        EXPECT_OK(result.status());
        EXPECT_OK(result.value().s);
        started.Signal();
      });
  started.Wait();
}

// If the NetworkDeviceImplStart has not been called, the IGC driver is not in the state which is
// ready for tx, and it will report ZX_ERR_UNAVAILABLE as the status for all the packets that higher
// layer wanted to send. This test case verifies this behavior.
TEST_F(IgcInterfaceTest, NetworkDeviceImplQueueTxNotStarted) {
  constexpr size_t kTxBufferCount = 10;

  // Construct the tx buffer list.
  fidl::VectorView<netdriver::wire::TxBuffer> tx_buffers(arena_, kTxBufferCount);

  // Populate buffer id only, since the driver won't make use of the data section if not started.
  for (size_t i = 0; i < kTxBufferCount; i++) {
    tx_buffers[i] = CreateTxBuffer(i);
  }

  auto result = netdev_client_.buffer(arena_)->QueueTx(tx_buffers);
  ASSERT_OK(result.status());

  netdev_ifc_.WaitUntilCompleteTxCalled();

  const auto& tx_results = netdev_ifc_.TxResults();

  // Verify the tx results of all packet buffers.
  EXPECT_EQ(tx_results.size(), kTxBufferCount);
  for (size_t i = 0; i < kTxBufferCount; i++) {
    EXPECT_EQ(tx_results[i].id(), tx_buffers[i].id);
    EXPECT_EQ(tx_results[i].status(), ZX_ERR_UNAVAILABLE);
  }
}

// This test case verifies the normal tx behavior of the IGC driver.
TEST_F(IgcInterfaceTest, NetworkDeviceImplQueueTxStarted) {
  // Pass two buffers in total in this test.
  constexpr size_t kTxBufferCount = 2;
  // Each buffer only contains 1 buffer region.
  constexpr size_t kBufferRegionCount = 1;

  // Construct the tx buffer list.
  fidl::VectorView<netdriver::wire::TxBuffer> tx_buffers(arena_, kTxBufferCount);

  constexpr size_t kFirstRegionLength = 50;
  constexpr size_t kSecondRegionLength = 1024;

  // First buffer.
  fidl::VectorView<netdriver::wire::BufferRegion> region_list_1(arena_, kBufferRegionCount);
  region_list_1[0].vmo = kVmoId;
  region_list_1[0].offset = 0;
  region_list_1[0].length = kFirstRegionLength;

  tx_buffers[0] = CreateTxBuffer(0);
  tx_buffers[0].data = region_list_1;

  // Second buffer.
  fidl::VectorView<netdriver::wire::BufferRegion> region_list_2(arena_, kBufferRegionCount);
  region_list_2[0].vmo = kVmoId;
  region_list_2[0].offset = 0;
  region_list_2[0].length = kSecondRegionLength;

  tx_buffers[1] = CreateTxBuffer(1);
  tx_buffers[1].data = region_list_2;

  // Store the VMO into VmoStore.
  ASSERT_OK(
      netdev_client_.sync().buffer(arena_)->PrepareVmo(kVmoId, std::move(test_vmo_)).status());

  // Call start to change the state of driver.
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->Start().status());

  ClearNumTxTailUpdates();
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffers).status());
  WaitForTxTailUpdates(tx_buffers.size());

  driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
    // Get the access to adapter structure in the IGC driver.
    auto adapter = driver.Adapter();
    auto driver_tx_buffer_info = driver.TxBuffer();

    // Verify the data in tx descriptor ring.
    EXPECT_EQ(adapter->txh_ind, 0u);
    EXPECT_EQ(adapter->txt_ind, kTxBufferCount);
    EXPECT_EQ(adapter->txr_len, kTxBufferCount);
    igc_tx_desc* first_desc_entry = &adapter->txdr[0];
    EXPECT_EQ(first_desc_entry->lower.data,
              (uint32_t)(IGC_TXD_CMD_EOP | IGC_TXD_CMD_IFCS | IGC_TXD_CMD_RS | kFirstRegionLength));
    igc_tx_desc* second_desc_entry = &adapter->txdr[1];
    EXPECT_EQ(second_desc_entry->lower.data, (uint32_t)(IGC_TXD_CMD_EOP | IGC_TXD_CMD_IFCS |
                                                        IGC_TXD_CMD_RS | kSecondRegionLength));

    // Verify the buffer id stored in the internal tx buffer info list.
    EXPECT_EQ(driver_tx_buffer_info[0].buffer_id, 0U);
    EXPECT_EQ(driver_tx_buffer_info[1].buffer_id, 1U);
  });

  // Test the wrap around of ring index.
  fidl::VectorView<netdriver::wire::TxBuffer> tx_buffer_extra(arena_, ei::kEthTxDescRingCount);

  // All the extra buffers share one buffer region, the content doesn't matter.
  fidl::VectorView<netdriver::wire::BufferRegion> region_list_extra(arena_, kBufferRegionCount);
  region_list_extra[0].vmo = kVmoId;
  region_list_extra[0].offset = 0;
  region_list_extra[0].length = kFirstRegionLength;

  // Send a list of buffer with the count equals to kEthTxDescRingCount, so that the ring buffer
  // index will circle back to the same index.
  for (size_t i = 0; i < ei::kEthTxDescRingCount; i++) {
    tx_buffer_extra[i] = CreateTxBuffer(i + 2);
    tx_buffer_extra[i].data = region_list_extra;
  }

  ClearNumTxTailUpdates();
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffer_extra).status());
  WaitForTxTailUpdates(tx_buffer_extra.size());

  driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
    // Get the access to adapter structure in the IGC driver.
    auto adapter = driver.Adapter();
    adapter->osdep.mmio_buffer->Write32(0, 0);
    auto driver_tx_buffer_info = driver.TxBuffer();

    // The tail index of tx descriptor ring buffer stays the same.
    EXPECT_EQ(adapter->txt_ind, kTxBufferCount);
    // But the buffer id at the same slot has been changed.
    EXPECT_EQ(driver_tx_buffer_info[1].buffer_id, ei::kEthTxDescRingCount + 1);
    // And the length of the tx descriptor ring should equal the sum of the two QueueTx operations.
    // Technically this exceeds the maximum TX depth since this test doesn't respect the depth
    // indicated by the driver. The driver currently does not reject this since netdevice is
    // responsible for only queueing as many TX buffers as the device can handle.
    EXPECT_EQ(adapter->txr_len, ei::kEthTxDescRingCount + kTxBufferCount);
  });
}

TEST_F(IgcInterfaceTest, NetworkDeviceImplQueueRxSpace) {
  // Pass two buffers in total in this test.
  constexpr size_t kRxBufferCount = 2;

  fidl::VectorView<netdriver::wire::RxSpaceBuffer> rx_space_buffer(arena_, kRxBufferCount);

  netdriver::wire::BufferRegion buffer_region = {
      .vmo = kVmoId,
  };

  // Use the same buffer region for all buffers.
  for (size_t i = 0; i < kRxBufferCount; i++) {
    rx_space_buffer[i].id = static_cast<uint32_t>(i);
    rx_space_buffer[i].region = buffer_region;
  }

  // Store the VMO into VmoStore.
  ASSERT_OK(
      netdev_client_.sync().buffer(arena_)->PrepareVmo(kVmoId, std::move(test_vmo_)).status());

  ClearNumRxTailUpdates();
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer).status());
  WaitForRxTailUpdates(rx_space_buffer.size());

  driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
    // Get the access to adapter structure in the IGC driver.
    auto adapter = driver.Adapter();
    auto driver_rx_buffer_info = driver.RxBuffer();

    EXPECT_EQ(adapter->rxh_ind, 0u);
    EXPECT_EQ(adapter->rxt_ind, kRxBufferCount);
    EXPECT_EQ(adapter->rxr_len, kRxBufferCount);

    EXPECT_EQ(driver_rx_buffer_info[0].buffer_id, 0U);
    EXPECT_TRUE(driver_rx_buffer_info[0].available);
    EXPECT_EQ(driver_rx_buffer_info[1].buffer_id, 1U);
    EXPECT_TRUE(driver_rx_buffer_info[1].available);

    union igc_adv_rx_desc* first_desc_entry = &adapter->rxdr[0];
    EXPECT_EQ(first_desc_entry->wb.upper.status_error, 0U);
    union igc_adv_rx_desc* second_desc_entry = &adapter->rxdr[1];
    EXPECT_EQ(second_desc_entry->wb.upper.status_error, 0U);

    // Reset the availability states of the first two buffers.
    driver_rx_buffer_info[0].available = false;
    driver_rx_buffer_info[1].available = false;
  });

  // Test the wrap around of ring index.
  fidl::VectorView<netdriver::wire::RxSpaceBuffer> rx_space_buffer_extra(arena_,
                                                                         ei::kEthRxBufCount);

  // Send extra kEthRxBufCount of buffers.
  for (size_t i = 0; i < ei::kEthRxBufCount; i++) {
    rx_space_buffer_extra[i].id = static_cast<uint32_t>(i) + kRxBufferCount;
    rx_space_buffer_extra[i].region = buffer_region;
  }

  ClearNumRxTailUpdates();
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer_extra).status());
  WaitForRxTailUpdates(rx_space_buffer_extra.size());

  driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
    // Get the access to adapter structure in the IGC driver.
    auto adapter = driver.Adapter();
    auto driver_rx_buffer_info = driver.RxBuffer();

    // The tail index of rx descriptor ring buffer stays the same.
    EXPECT_EQ(adapter->rxt_ind, kRxBufferCount);
    // But the buffer id at the same slot has been changed.
    EXPECT_EQ(driver_rx_buffer_info[1].buffer_id, ei::kEthRxBufCount + 1);
    EXPECT_TRUE(driver_rx_buffer_info[1].available);
    // And the length of the rx descriptor ring should equal the sum of the two QueueRxSpace
    // operations. Technically this exceeds the maximum RX depth since this test doesn't respect the
    // depth indicated by the driver. The driver currently does not reject this since netdevice is
    // responsible for only queueing as many RX space buffers as the device can handle.
    EXPECT_EQ(adapter->rxr_len, ei::kEthRxBufCount + kRxBufferCount);
  });
}

// Stop function will return all the rx space buffers with null data, and will also reclaim all the
// tx buffers. This test verifies this behavior.
TEST_F(IgcInterfaceTest, NetworkDeviceImplStop) {
  /* Prepare rx space buffers*/
  constexpr size_t kRxBufferCount = 5;

  fidl::VectorView<netdriver::wire::RxSpaceBuffer> rx_space_buffer(arena_, kRxBufferCount);

  netdriver::wire::BufferRegion buffer_region = {
      .vmo = kVmoId,
  };

  // Use the same buffer region for all buffers.
  for (size_t i = 0; i < kRxBufferCount; i++) {
    rx_space_buffer[i].id = static_cast<uint32_t>(i);
    rx_space_buffer[i].region = buffer_region;
  }

  /*Prepare tx buffers*/
  constexpr size_t kTxBufferCount = 5;
  constexpr size_t kBufferRegionCount = 1;

  // construct the tx buffer list.
  fidl::VectorView<netdriver::wire::TxBuffer> tx_buffer(arena_, kTxBufferCount);

  for (size_t i = 0; i < kTxBufferCount; i++) {
    // The buffer regions for each buffer. Assuming each buffer only contains one region in the
    // region list.
    fidl::VectorView<netdriver::wire::BufferRegion> regions(arena_, kBufferRegionCount);
    regions[i].vmo = kVmoId;
    regions[i].offset = 0;
    // Set the length of region to 10 times of the index, so that the 5 buffers contain regions with
    // length [0, 10, 20, 30, 40].
    regions[i].length = i * 10;

    tx_buffer[i] = CreateTxBuffer(i);
    tx_buffer[i].data = regions;
  }

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->Start().status());

  // Store the VMO into VmoStore.
  ASSERT_OK(
      netdev_client_.sync().buffer(arena_)->PrepareVmo(kVmoId, std::move(test_vmo_)).status());

  ClearNumRxTailUpdates();
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer).status());
  WaitForRxTailUpdates(rx_space_buffer.size());

  ClearNumTxTailUpdates();
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffer).status());
  WaitForTxTailUpdates(tx_buffer.size());

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->Stop().status());

  netdev_ifc_.WaitUntilCompleteRxCalled();
  netdev_ifc_.WaitUntilCompleteTxCalled();

  /*Verify the data comes from CompleteRx and CompleteTx*/

  // CompleteRx data
  const auto& rx_buffers = netdev_ifc_.RxBuffers();
  const auto& rx_buffer_parts = netdev_ifc_.RxBufferParts();
  EXPECT_EQ(rx_buffers.size(), kRxBufferCount);
  EXPECT_EQ(rx_buffers.size(), rx_buffer_parts.size());
  for (size_t n = 0; n < kRxBufferCount; n++) {
    EXPECT_EQ(rx_buffers[n].meta().port(), ei::kPortId);
    EXPECT_EQ(rx_buffers[n].meta().frame_type(), netdev::wire::FrameType::kEthernet);
    EXPECT_EQ(rx_buffers[n].data().size(), 1U);
    EXPECT_EQ(rx_buffer_parts[n].size(), 1U);
    EXPECT_EQ(rx_buffer_parts[n][0].id(), n);
    EXPECT_EQ(rx_buffer_parts[n][0].length(), 0U);
    EXPECT_EQ(rx_buffer_parts[n][0].offset(), 0U);
  }

  // CompleteTx data
  const auto& tx_results = netdev_ifc_.TxResults();
  EXPECT_EQ(tx_results.size(), kTxBufferCount);
  for (size_t n = 0; n < kTxBufferCount; n++) {
    EXPECT_EQ(tx_results[n].id(), n);
    EXPECT_EQ(tx_results[n].status(), ZX_ERR_UNAVAILABLE);
  }

  driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
    auto adapter = driver.Adapter();

    // The rx head index should stopped at the tail index of the rx descriptor ring.
    EXPECT_EQ(adapter->rxh_ind, adapter->rxt_ind);
    // The length of the rx descriptor ring should indicate that it's empty.
    EXPECT_EQ(adapter->rxr_len, 0u);

    // The tx head index should stopped at the tail index of the tx descriptor ring.
    EXPECT_EQ(adapter->txh_ind, adapter->txt_ind);
    // The length of the tx descriptor ring should indicate that it's empty.
    EXPECT_EQ(adapter->txr_len, 0u);
  });
}

// NetworkPort protocol tests
TEST_F(IgcInterfaceTest, NetworkPortGetInfo) {
  auto info_result = port_client_.sync().buffer(arena_)->GetInfo();
  ASSERT_OK(info_result.status());

  netdev::wire::PortBaseInfo& info = info_result->info;

  EXPECT_EQ(info.port_class(), netdev::wire::PortClass::kEthernet);
  EXPECT_EQ(info.rx_types()[0], netdev::wire::FrameType::kEthernet);
  EXPECT_EQ(info.rx_types().size(), 1U);
  EXPECT_EQ(info.tx_types()[0].type, netdev::wire::FrameType::kEthernet);
  EXPECT_EQ(info.tx_types()[0].features, netdev::wire::kFrameFeaturesRaw);
  EXPECT_EQ(info.tx_types()[0].supported_flags, netdev::wire::TxFlags{});
  EXPECT_EQ(info.tx_types().size(), 1U);
}

TEST_F(IgcInterfaceTest, NetworkPortGetStatus) {
  auto status_result = port_client_.sync().buffer(arena_)->GetStatus();
  ASSERT_OK(status_result.status());

  netdev::wire::PortStatus& status = status_result->status;

  EXPECT_EQ(status.mtu(), ei::kEtherMtu);
  EXPECT_EQ(status.flags(), netdev::wire::StatusFlags{});
}

TEST_F(IgcInterfaceTest, NetworkPortGetMac) {
  auto mac_result = port_client_.sync().buffer(arena_)->GetMac();
  ASSERT_OK(mac_result.status());

  ASSERT_TRUE(mac_result->mac_ifc.is_valid());
}

constexpr uint8_t kFakeMacAddr[ei::kEtherAddrLen] = {7, 7, 8, 9, 3, 4};
// MacAddr protocol tests
TEST_F(IgcInterfaceTest, MacAddrGetAddress) {
  driver_test().RunInDriverContext([&](ei::IgcDriver& driver) {
    // Get the address of driver adapter.
    auto adapter = driver.Adapter();
    // Set the MAC address to the driver manually.
    memcpy(adapter->hw.mac.addr, kFakeMacAddr, ei::kEtherAddrLen);
  });

  auto mac_result = port_client_.sync().buffer(arena_)->GetMac();
  ASSERT_OK(mac_result.status());

  auto mac = fdf::WireCall(mac_result->mac_ifc).buffer(arena_)->GetAddress();
  ASSERT_OK(mac.status());

  EXPECT_EQ(memcmp(mac->mac.octets.data(), kFakeMacAddr, ei::kEtherAddrLen), 0);
}

TEST_F(IgcInterfaceTest, MacAddrGetFeatures) {
  auto mac_result = port_client_.sync().buffer(arena_)->GetMac();
  ASSERT_OK(mac_result.status());

  auto features_result = fdf::WireCall(mac_result->mac_ifc).buffer(arena_)->GetFeatures();
  ASSERT_OK(features_result.status());

  const auto& features = features_result->features;

  EXPECT_EQ(features.multicast_filter_count(), 0U);
  EXPECT_EQ(features.supported_modes(), netdriver::wire::SupportedMacFilterMode::kPromiscuous);
}

}  // namespace
