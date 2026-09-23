// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <condition_variable>

#include <gtest/gtest.h>

#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/lib/testing/predicates/status.h"
#include "zircon/third_party/dev/ethernet/e1000/fuchsia.h"
#include "zircon/third_party/dev/ethernet/e1000/test/fake_mmio.h"

namespace {

namespace netdev = fuchsia_hardware_network;
namespace netdriver = fuchsia_hardware_network_driver;

constexpr size_t kFakeBarSize = 0x20000;
constexpr uint32_t kTestDeviceId = E1000_DEV_ID_PCH_SPT_I219_LM;
constexpr uint32_t kTestSubsysId = 0x0001;
constexpr uint32_t kTestSubsysVendorId = 0x0abe;
constexpr uint32_t kTestCommand = 0x0033;
constexpr uint8_t kVmoId = 0;
constexpr size_t kTxBufSize = 2048;

class NetworkDeviceIfc : public fdf::WireServer<netdriver::NetworkDeviceIfc> {
 public:
  explicit NetworkDeviceIfc(fdf_dispatcher* dispatcher) : dispatcher_(dispatcher) {
    // NetworkDeviceIfc must run on a different dispatcher to avoid deadlocks and to allow this
    // constructor to post a task and wait on it.
    EXPECT_NE(fdf::Dispatcher::GetCurrent()->get(), dispatcher);

    // The binding has to be created on the same dispatcher it is served on.
    libsync::Completion binding_created;
    EXPECT_OK(async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_), [&] {
      auto endpoints = fdf::CreateEndpoints<netdriver::NetworkDeviceIfc>();
      EXPECT_OK(endpoints.status_value());

      server_binding_.emplace(dispatcher_, std::move(endpoints->server), this,
                              fidl::kIgnoreBindingClosure);
      ifc_client_end_ = std::move(endpoints->client);
      binding_created.Signal();
    }));
    binding_created.Wait();
  }

  ~NetworkDeviceIfc() {
    if (server_binding_.has_value()) {
      EXPECT_NE(dispatcher_, nullptr);

      // The binding has to be destroyed on the same dispatcher it is served on.
      libsync::Completion binding_destroyed;
      EXPECT_OK(async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_), [&] {
        server_binding_.reset();
        binding_destroyed.Signal();
      }));
      binding_destroyed.Wait();
    }
  }

  fdf::ClientEnd<netdriver::NetworkDeviceIfc> TakeIfcClientEnd() {
    return std::move(ifc_client_end_);
  }
  fdf::ClientEnd<netdriver::NetworkPort> TakePortClientEnd() { return std::move(port_client_end_); }

  void WaitUntilCompleteRxCalled() {
    complete_rx_called_.Wait();
    complete_rx_called_.Reset();
  }
  void WaitUntilCompleteTxCalled() {
    complete_tx_called_.Wait();
    complete_tx_called_.Reset();
  }
  void ResetPortStatusChangedCalled() { port_status_changed_called_.Reset(); }
  void WaitUntilPortStatusChangedCalled() {
    port_status_changed_called_.Wait();
    port_status_changed_called_.Reset();
  }
  size_t NumPortStatusChangedCalls() const { return num_port_status_changed_calls_; }

  const auto& TxResults() const { return tx_results_; }
  const auto& RxBuffers() const { return rx_buffers_; }
  const auto& RxBufferParts() const { return rx_buffer_parts_; }
  netdev::PortStatus PortStatus() const { return port_status_; }

  // NetworkDeviceIfc implementation
  void PortStatusChanged(netdriver::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                         fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) override {
    ASSERT_EQ(request->id, e1000::kPortId);
    port_status_ = fidl::ToNatural(request->new_status);
    ++num_port_status_changed_calls_;
    port_status_changed_called_.Signal();
  }

  void AddPort(netdriver::wire::NetworkDeviceIfcAddPortRequest* request, fdf::Arena& arena,
               AddPortCompleter::Sync& completer) override {
    port_client_end_ = std::move(request->port);
    EXPECT_EQ(request->id, e1000::kPortId);
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
  fdf_dispatcher_t* dispatcher_ = nullptr;
  std::optional<fdf::ServerBinding<netdriver::NetworkDeviceIfc>> server_binding_;
  fdf::ClientEnd<netdriver::NetworkDeviceIfc> ifc_client_end_;
  fdf::ClientEnd<netdriver::NetworkPort> port_client_end_;

  libsync::Completion complete_tx_called_;
  libsync::Completion complete_rx_called_;
  std::atomic<size_t> num_port_status_changed_calls_ = 0;
  libsync::Completion port_status_changed_called_;

  // The vector to record tx results for each packets from CompleteTx.
  std::vector<netdriver::TxResult> tx_results_;

  // The vector to record rx buffers received from CompleteRx.
  std::vector<netdriver::RxBuffer> rx_buffers_;

  // The local copy of buffer parts received from CompleteRx.
  std::vector<std::vector<netdriver::RxBufferPart>> rx_buffer_parts_;

  netdev::PortStatus port_status_;
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

class FakeMmio : public e1000::test::MmioInterceptor {
 public:
  FakeMmio() = default;

  void OnMmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) override {
    if (mmio_base_ != 0) {
      uintptr_t offset = reinterpret_cast<uintptr_t>(buffer) - mmio_base_;
      if (offset == E1000_TDT(0)) {
        // This is the offset used for QueueTx.
        std::lock_guard lock(tx_tail_mutex_);
        ++num_tx_tail_updates_;
        tx_tail_updated_.notify_all();
      } else if (offset == E1000_RDT(0)) {
        // This is the offset used for QueueRxSpace.
        std::lock_guard lock(rx_tail_mutex_);
        ++num_rx_tail_updates_;
        rx_tail_updated_.notify_all();
      }
    }
    *(uint32_t*)buffer = data;
  }

  void WaitForTxTailUpdates(size_t num_updates_to_wait_for = 1) {
    std::unique_lock lock(tx_tail_mutex_);
    while (num_tx_tail_updates_ != num_updates_to_wait_for) {
      tx_tail_updated_.wait(lock);
    }
    num_tx_tail_updates_ = 0;
  }

  void WaitForRxTailUpdates(size_t num_updates_to_wait_for = 1) {
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

  void SetMemBase(uintptr_t mem_base) { mmio_base_ = mem_base; }

 private:
  std::atomic<uintptr_t> mmio_base_ = 0;

  std::condition_variable tx_tail_updated_;
  size_t num_tx_tail_updates_ = 0;
  std::mutex tx_tail_mutex_;

  std::condition_variable rx_tail_updated_;
  size_t num_rx_tail_updates_ = 0;
  std::mutex rx_tail_mutex_;
};

// We want to receive notifications when the interrupt is acked, override this class and provide our
// own implementation of AckInterrupt.
class FakePci : public pci::FakePciProtocol {
 public:
  zx::result<> AddService(fdf::OutgoingDirectory& outgoing) {
    return outgoing.AddService<fuchsia_hardware_pci::Service>(
        fuchsia_hardware_pci::Service::InstanceHandler(
            {.device = bind_handler(fdf::Dispatcher::GetCurrent()->async_dispatcher())}));
  }

  void AckInterrupt(AckInterruptCompleter::Sync& completer) override {
    pci::FakePciProtocol::AckInterrupt(completer);
    interrupt_acked_.Signal();
  }

  void WaitUntilInterruptAcked() {
    interrupt_acked_.Wait();
    interrupt_acked_.Reset();
  }

 private:
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_pci::Device>> binding_;
  libsync::Completion interrupt_acked_;
};

class TestFixtureEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing) override {
    device_server_.Initialize(component::kDefaultInstance);
    zx_status_t status =
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &outgoing);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    e1000::test::SetMmioInterceptor(&fake_mmio_);

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

    interrupt_ = &fake_pci_.AddLegacyInterrupt();

    if (zx::result result = fake_pci_.AddService(outgoing); result.is_error()) {
      return result;
    }
    return zx::ok();
  }

  FakePci fake_pci_;
  FakeMmio fake_mmio_;
  zx::interrupt* interrupt_ = nullptr;
  compat::DeviceServer device_server_;
};

struct TestFixtureConfig {
  using DriverType = e1000::Driver;
  using EnvironmentType = TestFixtureEnvironment;
};

class E1000Test : public ::testing::Test {
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

    auto info_result = netdev_client_.sync().buffer(arena_)->GetInfo();
    ASSERT_OK(info_result.status());
    const netdriver::wire::DeviceImplInfo& info = info_result.value().info;

    ASSERT_TRUE(info.has_rx_depth());
    ASSERT_TRUE(info.has_min_rx_buffer_length());
    // Create the VMO like what netdev driver does.
    zx::vmo::create(info.rx_depth() * info.min_rx_buffer_length() + info.tx_depth() * kTxBufSize,
                    kVmoId, &test_vmo_);

    driver_test().RunInEnvironmentTypeContext(
        [this](TestFixtureEnvironment& env) { fake_mmio_ = &env.fake_mmio_; });

    driver_test().RunInDriverContext([&](e1000::Driver& driver) {
      adapter_ = driver.Adapter();

      // Create two rings that shadow the actual TX and RX descriptor rings. These rings will NOT
      // contain the proper buffer IDs since they are contained in the rings objects owned by the
      // driver. But they will observe the same memory that those rings use for descriptors.
      fake_mmio_->SetMemBase(reinterpret_cast<uintptr_t>(hw2membase(&adapter_->hw)));
      void* descriptor_mem = io_buffer_virt(&adapter_->buffer);
      shadow_rx_ring_.AssignDescriptorMmio(descriptor_mem);
      descriptor_mem = reinterpret_cast<e1000_rx_desc_extended*>(descriptor_mem) + e1000::kRxDepth;
      shadow_tx_ring_.AssignDescriptorMmio(descriptor_mem);
    });
  }

  void TearDown() override { ASSERT_OK(driver_test().StopDriver().status_value()); }

  void WaitForTxTailUpdates(size_t num_updates_to_wait_for = 1) const {
    fake_mmio_->WaitForTxTailUpdates(num_updates_to_wait_for);
  }

  void WaitForRxTailUpdates(size_t num_updates_to_wait_for = 1) const {
    fake_mmio_->WaitForRxTailUpdates(num_updates_to_wait_for);
  }

  // Start the data path and set the device status to online.
  void StartDataPath() {
    // Call start to change the state of driver.
    ASSERT_OK(netdev_client_.sync().buffer(arena_)->Start().status());

    // Ensure the device is online or QueueTx won't accept the buffers.
    TriggerLinkStatusChange(true);

    // Start will have written the TX and RX tails, reset the counters.
    fake_mmio_->ClearNumTxTailUpdates();
    fake_mmio_->ClearNumRxTailUpdates();
  }

  bool IsOnline() const {
    auto status = port_client_.sync().buffer(arena_)->GetStatus();
    EXPECT_OK(status.status());
    EXPECT_TRUE(status->status.has_flags());
    return static_cast<bool>(status->status.flags() & netdev::StatusFlags::kOnline);
  }

  // Trigger a link status change interrupt indicating the given online state. The call will wait
  // for the entire sequence to complete, including the call to PortStatusChanged. When the call
  // completes the driver should be in an idle state and all observable state should reflect the
  // requested online state.
  void TriggerLinkStatusChange(bool link_online) {
    const bool initial_online_status = IsOnline();
    netdev_ifc_.ResetPortStatusChangedCalled();

    FakePci* pci = nullptr;
    // Triggering the interrupt that should cause the link status to change.
    driver_test().RunInEnvironmentTypeContext([&](TestFixtureEnvironment& env) {
      // Set the link-up bit in the status register.
      E1000_WRITE_REG(&adapter_->hw, E1000_STATUS, link_online ? E1000_STATUS_LU : 0);
      // Set interrupt status to indicate a link status change.
      E1000_WRITE_REG(&adapter_->hw, E1000_ICR, E1000_ICR_LSC);
      env.interrupt_->trigger(0, zx::time_boot(zx_clock_get_boot()));

      // Store a pointer to the fake PCI for waiting. We can't wait on the environment dispatcher
      // because that's what the fake PCI uses for serving the ack request.
      pci = &env.fake_pci_;
    });
    // Wait until the interrupt has been acked, this ensures that all processing has already
    // happened when this return. This makes it easier to inspect the state of the driver.
    pci->WaitUntilInterruptAcked();

    if (initial_online_status != link_online) {
      // If the state changed we should receive a port status changed call.
      netdev_ifc_.WaitUntilPortStatusChangedCalled();
    }

    driver_test().RunInDriverContext([this, link_online](e1000::Driver&) {
      // This runs on the driver dispatcher and it was scheduled after the interrupt was acked. This
      // means that this should have been scheduled after the driver scheduled its link status
      // handler. So by the time this runs, the link status handler should have completed. This
      // ensures that all handling of the link state change has completed when this method returns.

      // The port status reported through the netdev ifc (set by PortStatusChanged) should match the
      // requested state at this point.
      EXPECT_EQ(*netdev_ifc_.PortStatus().flags(),
                link_online ? netdev::StatusFlags::kOnline : netdev::StatusFlags{});
      // And if we query the driver's status it should also report a matching status.
      EXPECT_EQ(IsOnline(), link_online);
    });
  }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig>& driver_test() { return driver_test_; }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig> driver_test_;

  fdf::Arena arena_{'E1KT'};
  NetworkDeviceIfc netdev_ifc_{driver_test().runtime().StartBackgroundDispatcher()->get()};
  fdf::WireSharedClient<netdriver::NetworkDeviceImpl> netdev_client_;
  fdf::WireSharedClient<netdriver::NetworkPort> port_client_;

  FakeMmio* fake_mmio_ = nullptr;
  e1000::adapter* adapter_ = nullptr;

  e1000::TxRing<e1000::kTxDepth> shadow_tx_ring_;
  e1000::RxRing<e1000::kRxDepth, e1000_rx_desc_extended> shadow_rx_ring_;

  // The VMO faked by this test class to simulate the one that netdevice creates in real case.
  zx::vmo test_vmo_;
};

TEST_F(E1000Test, NetworkDeviceImplGetInfo) {
  auto result = netdev_client_.sync().buffer(arena_)->GetInfo();
  ASSERT_OK(result.status());

  netdriver::wire::DeviceImplInfo& info = result.value().info;

  EXPECT_EQ(info.tx_depth(), e1000::kTxDepth);
  EXPECT_EQ(info.rx_depth(), e1000::kRxDepth);
  EXPECT_EQ(info.rx_threshold(), e1000::kRxDepth / 2);
  EXPECT_EQ(info.max_buffer_parts(), 1U);
  EXPECT_EQ(info.max_buffer_length(), e1000::kMaxBufferLength);
  EXPECT_EQ(info.buffer_alignment(), e1000::kBufferAlignment);
  EXPECT_EQ(info.min_rx_buffer_length(), e1000::kMinRxBufferLength);
  EXPECT_EQ(info.min_tx_buffer_length(), e1000::kMinTxBufferLength);
  EXPECT_EQ(info.tx_head_length(), 0U);
  EXPECT_EQ(info.tx_tail_length(), 0U);
}

TEST_F(E1000Test, NetworkDeviceImplPrepareVmo) {
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

TEST_F(E1000Test, NetworkDeviceImplStart) {
  libsync::Completion started;
  netdev_client_.buffer(arena_)->Start().Then(
      [&](fdf::WireUnownedResult<netdriver::NetworkDeviceImpl::Start>& result) {
        EXPECT_OK(result.status());
        EXPECT_OK(result.value().s);
        started.Signal();
      });
  started.Wait();
}

// If the NetworkDeviceImplStart has not been called, the driver is not in the state which is ready
// for tx, and it will report ZX_ERR_UNAVAILABLE as the status for all the packets that higher layer
// wanted to send. This test case verifies this behavior.
TEST_F(E1000Test, NetworkDeviceImplQueueTxNotStarted) {
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

// If the device is not online the driver is not in the state which is ready for tx, and it will
// report ZX_ERR_UNAVAILABLE as the status for all the packets that higher layer wanted to send.
// This test case verifies this behavior.
TEST_F(E1000Test, NetworkDeviceImplQueueTxOffline) {
  constexpr size_t kTxBufferCount = 10;

  // Construct the tx buffer list.
  fidl::VectorView<netdriver::wire::TxBuffer> tx_buffers(arena_, kTxBufferCount);

  // Populate buffer id only, since the driver won't make use of the data section if not started.
  for (size_t i = 0; i < kTxBufferCount; i++) {
    tx_buffers[i] = CreateTxBuffer(i);
  }

  // Start the netdev data path but leave the device offline.
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->Start().status());

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

// This test case verifies the normal tx behavior of the driver.
TEST_F(E1000Test, NetworkDeviceImplQueueTxWhenStarted) {
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

  StartDataPath();

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffers).status());
  WaitForTxTailUpdates(kTxBufferCount);

  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    // Verify the data in tx descriptor ring.
    EXPECT_EQ(shadow_tx_ring_.Desc(0).lower.data,
              (E1000_TXD_CMD_EOP | E1000_TXD_CMD_IFCS | E1000_TXD_CMD_RS | kFirstRegionLength));
    EXPECT_EQ(shadow_tx_ring_.Desc(1).lower.data,
              (E1000_TXD_CMD_EOP | E1000_TXD_CMD_IFCS | E1000_TXD_CMD_RS | kSecondRegionLength));
    // Third entry should not have been modified
    EXPECT_EQ(shadow_tx_ring_.Desc(2).lower.data, 0u);
  });

  // Test the wrap around of ring index.
  fidl::VectorView<netdriver::wire::TxBuffer> tx_buffer_extra(arena_, e1000::kTxDepth);

  fidl::VectorView<netdriver::wire::BufferRegion> region_list_extra(arena_, e1000::kTxDepth);
  for (size_t i = 0; i < region_list_extra.size(); ++i) {
    region_list_extra[i].vmo = kVmoId;
    region_list_extra[i].offset = 0;
    region_list_extra[i].length = kFirstRegionLength + i;
  }

  // Send a list of buffer with the count equals to kEthTxDescRingCount, so that the ring buffer
  // index will circle back to the same index.
  for (size_t i = 0; i < tx_buffer_extra.size(); i++) {
    fidl::VectorView<netdriver::wire::BufferRegion> region(arena_, 1u);
    region[0].vmo = kVmoId;
    region[0].offset = 0;
    region[0].length = kFirstRegionLength + i;

    tx_buffer_extra[i] = CreateTxBuffer(i + 2);
    tx_buffer_extra[i].data = region;
  }

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffer_extra).status());
  WaitForTxTailUpdates(e1000::kTxDepth);

  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    for (size_t i = 0; i < e1000::kTxDepth; ++i) {
      // Start looking at the first entry of the last queue call.
      size_t index = (i + kTxBufferCount) & (e1000::kTxDepth - 1);
      e1000_tx_desc& desc = shadow_tx_ring_.Desc(index);

      EXPECT_EQ(desc.lower.data, E1000_TXD_CMD_EOP | E1000_TXD_CMD_IFCS | E1000_TXD_CMD_RS |
                                     (kFirstRegionLength + i));
    }
  });
}

TEST_F(E1000Test, NetworkDeviceImplQueueRxSpace) {
  // Pass two buffers in total in this test.
  constexpr size_t kRxBufferCount = 2;

  fidl::VectorView<netdriver::wire::RxSpaceBuffer> rx_space_buffer(arena_, kRxBufferCount);

  netdriver::wire::BufferRegion buffer_region = {.vmo = kVmoId, .length = e1000::kEthMtu};

  // Use the same buffer region for all buffers.
  for (size_t i = 0; i < kRxBufferCount; i++) {
    rx_space_buffer[i].id = static_cast<uint32_t>(i);
    rx_space_buffer[i].region = buffer_region;
  }

  // Store the VMO into VmoStore.
  ASSERT_OK(
      netdev_client_.sync().buffer(arena_)->PrepareVmo(kVmoId, std::move(test_vmo_)).status());

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer).status());
  WaitForRxTailUpdates(kRxBufferCount);

  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    // Get the access to adapter structure in the driver.
    const e1000_rx_desc_extended& first_desc_entry = shadow_rx_ring_.Desc(0);
    EXPECT_NE(first_desc_entry.read.buffer_addr, 0u);
    EXPECT_EQ(first_desc_entry.wb.upper.status_error, 0u);
    const e1000_rx_desc_extended& second_desc_entry = shadow_rx_ring_.Desc(1);
    EXPECT_NE(second_desc_entry.read.buffer_addr, 0u);
    EXPECT_EQ(second_desc_entry.wb.upper.status_error, 0u);
    // The third entry should not have been modified, it should have a buffer addr of zero.
    const e1000_rx_desc_extended& third_desc_entry = shadow_rx_ring_.Desc(2);
    EXPECT_EQ(third_desc_entry.read.buffer_addr, 0u);
  });

  // Test the wrap around of ring index.
  fidl::VectorView<netdriver::wire::RxSpaceBuffer> rx_space_buffer_extra(arena_, e1000::kRxDepth);

  // Send extra kEthRxBufCount of buffers.
  for (size_t i = 0; i < e1000::kRxDepth; i++) {
    rx_space_buffer_extra[i].id = static_cast<uint32_t>(i) + kRxBufferCount;
    rx_space_buffer_extra[i].region.offset = i;
    rx_space_buffer_extra[i].region.length = e1000::kEthMtu;
  }

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer_extra).status());
  WaitForRxTailUpdates(e1000::kRxDepth);

  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    uint64_t base_addr = shadow_rx_ring_.Desc(kRxBufferCount).read.buffer_addr;

    for (size_t i = 0; i < e1000::kRxDepth; ++i) {
      // Start looking at the first entry of the last queue call.
      size_t index = (i + kRxBufferCount) & (e1000::kRxDepth - 1);
      e1000_rx_desc_extended& desc = shadow_rx_ring_.Desc(index);
      EXPECT_EQ(desc.read.buffer_addr, base_addr + i);
      EXPECT_EQ(desc.wb.upper.status_error, 0u);
    }
  });
}

TEST_F(E1000Test, InterruptCompletesTx) {
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

  StartDataPath();

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffers).status());
  WaitForTxTailUpdates(kTxBufferCount);

  // First mark all buffers as completed.
  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    // Indicate that all TX buffer descriptors are written back.
    for (size_t i = 0; i < tx_buffers.size(); ++i) {
      shadow_tx_ring_.Desc(i).upper.fields.status = E1000_TXD_STAT_DD;
    }
  });

  // Triggering the interrupt that should cause the bufers to be completed.
  driver_test().RunInEnvironmentTypeContext([this](TestFixtureEnvironment& env) {
    // Set interrupt status to indicate TX descriptor write back.
    E1000_WRITE_REG(&adapter_->hw, E1000_ICR, E1000_ICR_TXDW);
    env.interrupt_->trigger(0, zx::time_boot(zx_clock_get_boot()));
  });

  netdev_ifc_.WaitUntilCompleteTxCalled();

  const auto& tx_results = netdev_ifc_.TxResults();

  // Verify the tx results of all packet buffers.
  EXPECT_EQ(tx_results.size(), kTxBufferCount);
  for (size_t i = 0; i < kTxBufferCount; i++) {
    EXPECT_EQ(tx_results[i].id(), tx_buffers[i].id);
    EXPECT_EQ(tx_results[i].status(), ZX_OK);
  }
}

TEST_F(E1000Test, InterruptCompletesRx) {
  // Pass two buffers in total in this test.
  constexpr size_t kRxBufferCount = 2;
  constexpr size_t kRxBufferSize = 2048;

  fidl::VectorView<netdriver::wire::RxSpaceBuffer> rx_space_buffer(arena_, kRxBufferCount);

  for (size_t i = 0; i < kRxBufferCount; i++) {
    rx_space_buffer[i].id = static_cast<uint32_t>(i);
    rx_space_buffer[i].region = {
        .vmo = kVmoId,
        .offset = i * kRxBufferSize,
        .length = kRxBufferSize,
    };
  }

  // Store the VMO into VmoStore.
  ASSERT_OK(
      netdev_client_.sync().buffer(arena_)->PrepareVmo(kVmoId, std::move(test_vmo_)).status());

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer).status());
  WaitForRxTailUpdates(kRxBufferCount);

  // Mark some buffers as received. Mark one more buffer than available as ready to make sure the
  // driver only returns buffers that it has RX space for.
  driver_test().RunInDriverContext([this](e1000::Driver& driver) {
    struct e1000::adapter* adapter = driver.Adapter();
    std::lock_guard lock(adapter->rx_mutex);

    for (size_t i = 0; i < kRxBufferCount + 1; ++i) {
      e1000_rx_desc_extended& desc = shadow_rx_ring_.Desc(i);
      desc.wb.upper.status_error = E1000_RXD_STAT_DD;
      desc.wb.upper.length = kRxBufferSize;
    }
  });

  // Triggering the interrupt that should cause buffers to be completed.
  driver_test().RunInEnvironmentTypeContext([this](TestFixtureEnvironment& env) {
    // Set interrupt status to indicate RX timer interrupt.
    E1000_WRITE_REG(&adapter_->hw, E1000_ICR, E1000_ICR_RXT0);
    env.interrupt_->trigger(0, zx::time_boot(zx_clock_get_boot()));
  });

  netdev_ifc_.WaitUntilCompleteRxCalled();

  const std::vector<netdriver::RxBuffer>& rx_buffers = netdev_ifc_.RxBuffers();
  const std::vector<std::vector<netdriver::RxBufferPart>>& rx_buffer_parts =
      netdev_ifc_.RxBufferParts();

  ASSERT_EQ(rx_buffers.size(), kRxBufferCount);
  ASSERT_EQ(rx_buffer_parts.size(), kRxBufferCount);

  for (size_t n = 0; n < kRxBufferCount; n++) {
    EXPECT_EQ(rx_buffers[n].meta().port(), e1000::kPortId);
    EXPECT_EQ(rx_buffers[n].meta().frame_type(), netdev::wire::FrameType::kEthernet);
    // Each buffer should only have one buffer part.
    EXPECT_EQ(rx_buffers[n].data().size(), 1u);
    EXPECT_EQ(rx_buffer_parts[n].size(), 1u);
    EXPECT_EQ(rx_buffer_parts[n][0].id(), n);
    EXPECT_EQ(rx_buffer_parts[n][0].length(), kRxBufferSize);
    EXPECT_EQ(rx_buffer_parts[n][0].offset(), 0u);
  }
}

TEST_F(E1000Test, InterruptChangesLinkStatus) {
  // Ensure device starts out offline.
  ASSERT_FALSE(IsOnline());

  // Call start to enable interrupts.
  ASSERT_OK(netdev_client_.sync().buffer(arena_)->Start().status());

  auto verify_port_status_changed = [this](netdev::StatusFlags expected_status) {
    netdev::PortStatus port_status = netdev_ifc_.PortStatus();
    EXPECT_EQ(port_status.mtu(), e1000::kEthMtu);
    EXPECT_EQ(port_status.flags(), expected_status);
  };

  TriggerLinkStatusChange(true);
  verify_port_status_changed(netdev::StatusFlags::kOnline);
  EXPECT_EQ(netdev_ifc_.NumPortStatusChangedCalls(), 1u);

  // Setting the link to the same status should not call port status changed, verify counter.
  TriggerLinkStatusChange(true);
  EXPECT_EQ(netdev_ifc_.NumPortStatusChangedCalls(), 1u);

  // Going offline again should trigger the change.
  TriggerLinkStatusChange(false);
  verify_port_status_changed(netdev::StatusFlags{});
  EXPECT_EQ(netdev_ifc_.NumPortStatusChangedCalls(), 2u);

  // Setting the link to the same status should not call port status changed, verify counter later.
  TriggerLinkStatusChange(false);
  EXPECT_EQ(netdev_ifc_.NumPortStatusChangedCalls(), 2u);
}

// Stop function will return all the rx space buffers with null data, and will also reclaim all the
// tx buffers. This test verifies this behavior.
TEST_F(E1000Test, NetworkDeviceImplStop) {
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

  // Store the VMO into VmoStore.
  ASSERT_OK(
      netdev_client_.sync().buffer(arena_)->PrepareVmo(kVmoId, std::move(test_vmo_)).status());

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueRxSpace(rx_space_buffer).status());
  WaitForRxTailUpdates(kRxBufferCount);

  StartDataPath();

  ASSERT_OK(netdev_client_.sync().buffer(arena_)->QueueTx(tx_buffer).status());
  WaitForTxTailUpdates(kTxBufferCount);

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
    EXPECT_EQ(rx_buffers[n].meta().port(), e1000::kPortId);
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

  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    // All RX descriptor data should be cleared out.
    auto rx_ring_data = reinterpret_cast<const uint8_t*>(&shadow_rx_ring_.Desc(0));
    const size_t rx_ring_size = e1000::kRxDepth * sizeof(e1000_rx_desc_extended);
    EXPECT_TRUE(std::all_of(rx_ring_data, rx_ring_data + rx_ring_size,
                            [](uint8_t byte) { return byte == 0; }));

    // All TX descriptor data should be cleared out.
    auto tx_ring_data = reinterpret_cast<const uint8_t*>(&shadow_tx_ring_.Desc(0));
    const size_t tx_ring_size = e1000::kTxDepth * sizeof(e1000_tx_desc);
    EXPECT_TRUE(std::all_of(tx_ring_data, tx_ring_data + tx_ring_size,
                            [](uint8_t byte) { return byte == 0; }));
  });
}

// NetworkPort protocol tests
TEST_F(E1000Test, NetworkPortGetInfo) {
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

TEST_F(E1000Test, NetworkPortGetStatus) {
  // Verify status when offline
  auto status_result = port_client_.sync().buffer(arena_)->GetStatus();
  ASSERT_OK(status_result.status());

  EXPECT_EQ(status_result->status.mtu(), e1000::kEthMtu);
  EXPECT_EQ(status_result->status.flags(), netdev::wire::StatusFlags{});

  // Verify status when online
  TriggerLinkStatusChange(true);

  status_result = port_client_.sync().buffer(arena_)->GetStatus();
  ASSERT_OK(status_result.status());

  EXPECT_EQ(status_result->status.mtu(), e1000::kEthMtu);
  EXPECT_EQ(status_result->status.flags(), netdev::wire::StatusFlags::kOnline);
}

TEST_F(E1000Test, NetworkPortGetMac) {
  auto mac_result = port_client_.sync().buffer(arena_)->GetMac();
  ASSERT_OK(mac_result.status());

  ASSERT_TRUE(mac_result->mac_ifc.is_valid());
}

// MacAddr protocol tests
TEST_F(E1000Test, MacAddrGetAddress) {
  constexpr uint8_t kFakeMacAddr[ETHER_ADDR_LEN] = {0x11, 0x22, 0x33, 0x44, 0x55, 0x66};

  driver_test().RunInDriverContext([&](e1000::Driver& driver) {
    // Get the address of driver adapter.
    auto adapter = driver.Adapter();
    // Set the MAC address to the driver manually.
    memcpy(adapter->hw.mac.addr, kFakeMacAddr, ETHER_ADDR_LEN);
  });

  auto mac_result = port_client_.sync().buffer(arena_)->GetMac();
  ASSERT_OK(mac_result.status());

  auto mac = fdf::WireCall(mac_result->mac_ifc).buffer(arena_)->GetAddress();
  ASSERT_OK(mac.status());

  EXPECT_EQ(memcmp(mac->mac.octets.data(), kFakeMacAddr, ETHER_ADDR_LEN), 0);
}

TEST_F(E1000Test, MacAddrGetFeatures) {
  auto mac_result = port_client_.sync().buffer(arena_)->GetMac();
  ASSERT_OK(mac_result.status());

  auto features_result = fdf::WireCall(mac_result->mac_ifc).buffer(arena_)->GetFeatures();
  ASSERT_OK(features_result.status());

  const auto& features = features_result->features;

  EXPECT_EQ(features.multicast_filter_count(), e1000::kMaxMulticastFilters);
  EXPECT_EQ(features.supported_modes(),
            netdriver::wire::SupportedMacFilterMode::kMulticastFilter |
                netdriver::wire::SupportedMacFilterMode::kMulticastPromiscuous |
                netdriver::wire::SupportedMacFilterMode::kPromiscuous);
}

}  // namespace
