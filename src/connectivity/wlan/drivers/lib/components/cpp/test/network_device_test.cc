// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/stdcompat/span.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <wlan/drivers/components/frame_container.h>
#include <wlan/drivers/components/frame_storage.h>
#include <wlan/drivers/components/network_device.h>

#include "src/connectivity/wlan/drivers/lib/components/cpp/test/test_driver.h"
#include "src/connectivity/wlan/drivers/lib/components/cpp/test/test_network_device_ifc.h"
#include "src/lib/testing/predicates/status.h"

namespace {

using wlan::drivers::components::Frame;
using wlan::drivers::components::FrameContainer;
using wlan::drivers::components::FrameStorage;
using wlan::drivers::components::NetworkDevice;

using wlan::drivers::components::test::TestDriver;
using wlan::drivers::components::test::TestNetworkDeviceIfc;

using testing::_;

constexpr const char kDriverName[] = "test-driver";

// Test implementation of the NetworkDevice base class we're testing
struct TestNetworkDevice : public TestDriver::StopHandler, public NetworkDevice::Callbacks {
  TestNetworkDevice() {
    // Set up some default behavior for these mock methods so that tests don't have to mock the
    // most basic things unless they want to.
    ON_CALL(*this, NetDevInit).WillByDefault([](NetworkDevice::Callbacks::InitTxn txn) {
      txn.Reply(ZX_OK);
    });
    ON_CALL(*this, NetDevStart).WillByDefault([](NetworkDevice::Callbacks::StartTxn txn) {
      txn.Reply(ZX_OK);
    });
    ON_CALL(*this, NetDevStop).WillByDefault([](NetworkDevice::Callbacks::StopTxn txn) {
      txn.Reply();
    });
    ON_CALL(*this, NetDevPrepareVmo)
        .WillByDefault(
            [this](uint8_t vmo_id, zx::vmo vmo, uint8_t* mapped_addr, size_t mapped_size) {
              vmo_addrs_[vmo_id] = mapped_addr;
              return ZX_OK;
            });
  }

  void Start(fidl::WireClient<fuchsia_driver_framework::Node>& parent,
             fdf::OutgoingDirectory& outgoing) {
    auto netdev_dispatcher =
        fdf::UnsynchronizedDispatcher::Create({}, "netdev", [this](fdf_dispatcher_t*) {
          if (prepare_stop_completer_.has_value()) {
            (*prepare_stop_completer_)(zx::ok());
          } else {
            netdev_dispatcher_shutdown_.Signal();
          }
        });
    ASSERT_OK(netdev_dispatcher.status_value());
    netdev_dispatcher_ = std::move(netdev_dispatcher.value());

    ASSERT_OK(network_device_->Initialize(parent, netdev_dispatcher_.get(), outgoing, kDriverName));
  }

  void Stop(fdf::StopCompleter completer) override {
    if (network_device_ && network_device_->Remove()) {
      // Asynchronous removal of net device will happen. Let it call the prepare stop completer.
      prepare_stop_completer_.emplace(std::move(completer));
    } else {
      // Net device is already removed, wait for NetDevRelease to finish shutting down dispatcher
      // and then complete.
      netdev_dispatcher_shutdown_.Wait();
      completer(zx::ok());
    }
  }

  // Don't mock this, it needs to shut down the dispatcher properly to signal completion of
  // PrepareStop. Instead use an std::function for tests that need to mock or verify release calls.
  void NetDevRelease() override {
    if (on_release_) {
      on_release_();
    }
    release_called_.Signal();
    if (netdev_dispatcher_.get()) {
      netdev_dispatcher_.ShutdownAsync();
    }
  }
  MOCK_METHOD(void, NetDevInit, (NetworkDevice::Callbacks::InitTxn txn), (override));
  MOCK_METHOD(void, NetDevStart, (NetworkDevice::Callbacks::StartTxn txn), (override));
  MOCK_METHOD(void, NetDevStop, (NetworkDevice::Callbacks::StopTxn txn), (override));
  MOCK_METHOD(void, NetDevGetInfo, (fuchsia_hardware_network_driver::DeviceImplInfo * out_info),
              (override));
  MOCK_METHOD(void, NetDevQueueTx, (cpp20::span<Frame> buffers), (override));
  MOCK_METHOD(void, NetDevQueueRxSpace,
              (cpp20::span<const fuchsia_hardware_network_driver::wire::RxSpaceBuffer> buffers,
               uint8_t* vmo_addrs[]),
              (override));
  MOCK_METHOD(zx_status_t, NetDevPrepareVmo,
              (uint8_t vmo_id, zx::vmo vmo, uint8_t* mapped_addr, size_t mapped_size), (override));
  MOCK_METHOD(void, NetDevReleaseVmo, (uint8_t vmo_id), (override));

  std::function<void()> on_release_;
  libsync::Completion release_called_;
  std::array<uint8_t*, fuchsia_hardware_network::wire::kMaxDataVmos> vmo_addrs_ = {};

  fdf::Dispatcher netdev_dispatcher_;
  libsync::Completion netdev_dispatcher_shutdown_;
  std::optional<fdf::StopCompleter> prepare_stop_completer_;

  // Ownership is in a pointer so that it can be manually destroyed in tests if needed.
  std::unique_ptr<NetworkDevice> network_device_ = std::make_unique<NetworkDevice>(this);
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
    return zx::ok();
  }

 private:
  compat::DeviceServer device_server_;
};

struct TestFixtureConfig {
  using DriverType = TestDriver;
  using EnvironmentType = TestFixtureEnvironment;
};

struct BasicNetworkDeviceTest : public ::testing::Test {
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
    driver_test().RunInDriverContext([&](TestDriver& driver) {
      parent_.Bind(driver.take_node(), fdf::Dispatcher::GetCurrent()->async_dispatcher());
      ASSERT_TRUE(parent_.is_valid());

      // Keep track of the dispatcher used to initialize the network device.
      driver_dispatcher_ = fdf::Dispatcher::GetCurrent();

      test_network_device_.Start(parent_, *driver.outgoing());
      driver.AssignStopHandler(&test_network_device_);
    });

    zx::result netdev_impl =
        driver_test().Connect<fuchsia_hardware_network_driver::Service::NetworkDeviceImpl>();
    ASSERT_OK(netdev_impl.status_value());

    network_device_client_.Bind(std::move(netdev_impl.value()),
                                fdf::Dispatcher::GetCurrent()->get());
    ASSERT_TRUE(network_device_client_.is_valid());

    test_network_device_.network_device_->WaitUntilServerConnected();

    auto ifc_client_end =
        network_device_ifc_.Bind(driver_test().runtime().StartBackgroundDispatcher()->get());
    ASSERT_OK(ifc_client_end.status_value());
    ifc_client_end_ = std::move(ifc_client_end.value());
  }

  void TearDown() override {
    CallStopDriver();
    driver_test().RunInDriverContext([&](TestDriver& driver) {
      // Because parent_ was bound in the driver's context and therefore on the driver dispatcher it
      // has to be destroyed here, not on the dispatcher that the test's destructor will run on.
      parent_ = {};
    });
  }

  void CallStopDriver() {
    if (!stop_driver_called_) {
      ASSERT_OK(driver_test().StopDriver().status_value());
      stop_driver_called_ = true;
    }
  }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig>& driver_test() { return driver_test_; }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig> driver_test_;
  fdf::UnownedDispatcher driver_dispatcher_;

  fdf::WireClient<fuchsia_hardware_network_driver::NetworkDeviceImpl> network_device_client_;
  TestNetworkDevice test_network_device_;
  TestNetworkDeviceIfc network_device_ifc_;
  fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceIfc> ifc_client_end_;

  fidl::WireClient<fuchsia_driver_framework::Node> parent_;
  bool stop_driver_called_ = false;
};

TEST(NetworkDeviceTest, Constructible) {
  fdf_testing::DriverRuntime runtime;

  NetworkDevice device(nullptr);
}

TEST_F(BasicNetworkDeviceTest, InitRelease) {
  CallStopDriver();
  test_network_device_.release_called_.Wait();
}

TEST_F(BasicNetworkDeviceTest, PrepareVmo) {
  constexpr uint8_t kVmoId = 7;
  constexpr const char kTestData[] = "This is some test data, we're writing it to the VMO";

  zx::vmo vmo;
  // The VMO size will be rounded up to page size so just make sure we already have a multiple of it
  // so tests will pass regardless of page size.
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo::create(kVmoSize, 0, &vmo);

  ASSERT_EQ(vmo.write(kTestData, 0, sizeof(kTestData)), ZX_OK);

  EXPECT_CALL(test_network_device_, NetDevPrepareVmo(kVmoId, _, _, kVmoSize))
      .WillOnce([&](uint8_t vmo_id, zx::vmo vmo, uint8_t* mapped_addr, size_t mapped_size) {
        // We should be able to read our test data from the mapped address
        EXPECT_STREQ(reinterpret_cast<const char*>(mapped_addr), kTestData);
        return ZX_OK;
      });

  fdf::Arena arena('NETD');
  auto result = network_device_client_.sync().buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(result.status()) << result.FormatDescription();
  ASSERT_OK(result.value().s);
}

TEST_F(BasicNetworkDeviceTest, DestroyInReleaseCallback) {
  // Destroy the NetworkDevice object in NetDevRelease to verify that this doesn't crash.
  libsync::Completion released;
  test_network_device_.on_release_ = [&] {
    test_network_device_.network_device_.reset();
    released.Signal();
  };

  test_network_device_.network_device_->Remove();

  released.Wait();
}

TEST_F(BasicNetworkDeviceTest, PrepareInvalidVmo) {
  constexpr uint8_t kVmoId = 7;

  EXPECT_CALL(test_network_device_, NetDevPrepareVmo).Times(0);

  // Preparing an invalid VMO handle should result in an error.
  fdf::Arena arena('NETD');
  auto result =
      network_device_client_.sync().buffer(arena)->PrepareVmo(kVmoId, zx::vmo(ZX_HANDLE_INVALID));
  ASSERT_EQ(result.status(), ZX_ERR_INVALID_ARGS);
}

TEST_F(BasicNetworkDeviceTest, ReleaseVmo) {
  constexpr uint8_t kVmoId = 9;

  zx::vmo vmo;
  // The VMO size will be rounded up to page size so just make sure we already have a multiple of it
  // so tests will pass regardless of page size.
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo::create(kVmoSize, 0, &vmo);

  EXPECT_CALL(test_network_device_, NetDevPrepareVmo(kVmoId, _, _, _));

  fdf::Arena arena('NETD');
  auto prepare = network_device_client_.sync().buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_OK(prepare.status());
  ASSERT_OK(prepare.value().s);

  libsync::Completion release_vmo_called;
  EXPECT_CALL(test_network_device_, NetDevReleaseVmo(kVmoId)).WillOnce([&] {
    release_vmo_called.Signal();
  });
  auto release = network_device_client_.sync().buffer(arena)->ReleaseVmo(kVmoId);
  ASSERT_OK(release.status());
  release_vmo_called.Wait();
}

TEST_F(BasicNetworkDeviceTest, Start) {
  EXPECT_CALL(test_network_device_, NetDevStart)
      .WillOnce([&](NetworkDevice::Callbacks::StartTxn txn) { txn.Reply(ZX_OK); });

  fdf::Arena arena('NETD');
  auto start = network_device_client_.sync().buffer(arena)->Start();
  ASSERT_OK(start.status());
  ASSERT_OK(start.value().s);
}

TEST_F(BasicNetworkDeviceTest, Stop) {
  EXPECT_CALL(test_network_device_, NetDevStart)
      .WillOnce([&](NetworkDevice::Callbacks::StartTxn txn) { txn.Reply(ZX_OK); });
  libsync::Completion stop_called;
  EXPECT_CALL(test_network_device_, NetDevStop)
      .WillOnce([&](NetworkDevice::Callbacks::StopTxn txn) {
        txn.Reply();
        stop_called.Signal();
      });

  fdf::Arena arena('NETD');
  // Must Start before we can stop.
  auto start = network_device_client_.sync().buffer(arena)->Start();
  auto stop = network_device_client_.sync().buffer(arena)->Stop();
  ASSERT_OK(stop.status());
  stop_called.Wait();
}

TEST_F(BasicNetworkDeviceTest, NetDevIfcClient) {
  fdf::Arena arena('NETD');
  EXPECT_CALL(test_network_device_, NetDevInit).WillOnce([](NetworkDevice::Callbacks::InitTxn txn) {
    txn.Reply(ZX_OK);
  });
  auto init = network_device_client_.sync().buffer(arena)->Init(std::move(ifc_client_end_));
  ASSERT_OK(init.status());

  ASSERT_TRUE(test_network_device_.network_device_->NetDevIfcClient().is_valid());
}

struct NetworkDeviceTestFixture : public BasicNetworkDeviceTest {
  static constexpr uint8_t kVmoId = 13;
  static constexpr uint8_t kPortId = 8;

  void SetUp() override {
    BasicNetworkDeviceTest::SetUp();

    kVmoSize = zx_system_get_page_size();

    fdf::Arena arena('NETD');
    EXPECT_CALL(test_network_device_, NetDevInit);
    auto init = network_device_client_.sync().buffer(arena)->Init(std::move(ifc_client_end_));
    ASSERT_OK(init.status());
    ASSERT_TRUE(test_network_device_.network_device_->NetDevIfcClient().is_valid());

    // Some of the operations in NetworkDevice require an actual VMO, create and prepare one.
    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);
    EXPECT_CALL(test_network_device_, NetDevPrepareVmo(kVmoId, _, _, _));
    auto prepare = network_device_client_.sync().buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
    ASSERT_OK(prepare.status());
    ASSERT_OK(prepare.value().s);

    // Make sure the device starts.
    EXPECT_CALL(test_network_device_, NetDevStart)
        .WillOnce([&](NetworkDevice::Callbacks::StartTxn txn) { txn.Reply(ZX_OK); });
    auto start = network_device_client_.sync().buffer(arena)->Start();
    ASSERT_OK(start.status());
    ASSERT_OK(start.value().s);

    std::lock_guard lock(frame_storage_);
    frame_storage_.Store(CreateTestFrame(0, 256, &frame_storage_));
    frame_storage_.Store(CreateTestFrame(0, 256, &frame_storage_));
    frame_storage_.Store(CreateTestFrame(0, 256, &frame_storage_));
  }

  // In order to be able to use ASSERT it seems the method has to return void. So do this little
  // song and dance to have a method that's easy to use while being able to ASSERT that its
  // parameters are valid.
  Frame CreateTestFrame(size_t offset = 0, uint32_t size = 256, FrameStorage* storage = nullptr) {
    Frame frame;
    CreateTestFrameImpl(offset, size, storage, &frame);
    return frame;
  }

  static void FillTxBuffers(
      fdf::Arena& arena, fidl::VectorView<fuchsia_hardware_network_driver::wire::TxBuffer>& buffers,
      fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>& regions,
      size_t frame_size = 256, uint16_t head_length = 0, uint16_t tail_length = 0) {
    ASSERT_EQ(buffers.size(), regions.size());

    size_t vmo_offset = 0;
    uint32_t buffer_id = 0;
    for (size_t i = 0; i < buffers.size(); ++i) {
      regions[i].length = frame_size;
      regions[i].vmo = kVmoId;
      regions[i].offset = vmo_offset;
      vmo_offset += frame_size;
      buffers[i].data =
          fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
              &regions[i], 1u);
      buffers[i].head_length = head_length;
      buffers[i].tail_length = tail_length;
      buffers[i].meta.port = kPortId;
      buffers[i].meta.frame_type = ::fuchsia_hardware_network::wire::FrameType::kEthernet;
      buffers[i].id = buffer_id++;
    }
  }

  FrameContainer AcquireTestFrames(size_t count) {
    std::lock_guard lock(frame_storage_);
    return frame_storage_.Acquire(count);
  }

  size_t StorageSize() {
    std::lock_guard lock(frame_storage_);
    return frame_storage_.size();
  }

  size_t kVmoSize;
  FrameStorage frame_storage_;

 private:
  void CreateTestFrameImpl(size_t offset, uint32_t size, FrameStorage* storage, Frame* frame) {
    ASSERT_LE(offset + size, kVmoSize);
    static uint16_t buffer_id = 1;
    *frame = Frame(storage, kVmoId, offset, buffer_id++, test_network_device_.vmo_addrs_[kVmoId],
                   size, kPortId);
  }
};

TEST_F(NetworkDeviceTestFixture, CompleteTx) {
  FrameContainer frames = AcquireTestFrames(3);

  libsync::Completion complete_tx_called;
  network_device_ifc_.complete_tx_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
          fdf::Arena& arena, auto&) {
        const auto& results = request->tx;
        ASSERT_EQ(results.size(), 3u);
        EXPECT_EQ(results[0].id, frames[0].BufferId());
        EXPECT_EQ(results[0].status, ZX_OK);
        EXPECT_EQ(results[1].id, frames[1].BufferId());
        EXPECT_EQ(results[1].status, ZX_OK);
        EXPECT_EQ(results[2].id, frames[2].BufferId());
        EXPECT_EQ(results[2].status, ZX_OK);
        complete_tx_called.Signal();
      };

  fdf::Arena arena('NETD');
  test_network_device_.network_device_->CompleteTx(frames, ZX_OK);
  complete_tx_called.Wait();
}

TEST_F(NetworkDeviceTestFixture, CompleteTxInvalidVmos) {
  FrameContainer frames = AcquireTestFrames(3);

  // Replace with a frame with an invalid VMO id, this frame should not be completed
  frames[0] =
      Frame(nullptr, kVmoId + 1, 0, 100, test_network_device_.vmo_addrs_[kVmoId], 256, kPortId);

  // Make sure we don't accidentally picked the same buffer ID
  ASSERT_NE(frames[0].BufferId(), frames[1].BufferId());
  ASSERT_NE(frames[1].BufferId(), frames[2].BufferId());

  libsync::Completion completed_tx;
  network_device_ifc_.complete_tx_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
          fdf::Arena&, auto&) {
        // Size should only be 2, the first frame should not have completed
        const auto& results = request->tx;
        ASSERT_EQ(results.size(), 2u);
        EXPECT_EQ(results[0].id, frames[1].BufferId());
        EXPECT_EQ(results[0].status, ZX_OK);
        EXPECT_EQ(results[1].id, frames[2].BufferId());
        EXPECT_EQ(results[1].status, ZX_OK);
        completed_tx.Signal();
      };
  fdf::Arena arena('NETD');
  test_network_device_.network_device_->CompleteTx(frames, ZX_OK);
  completed_tx.Wait();
}

TEST_F(NetworkDeviceTestFixture, CompleteTxNoFrames) {
  FrameContainer frames;
  // Should not have been called because there were no frames to complete
  network_device_ifc_.complete_tx_ =
      [](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
         fdf::Arena&, auto&) { ADD_FAILURE() << "CompleteTx should NOT be called"; };

  test_network_device_.network_device_->CompleteTx(frames, ZX_OK);
}

TEST_F(NetworkDeviceTestFixture, CompleteTxStatusPropagates) {
  constexpr zx_status_t kStatus = ZX_ERR_NO_RESOURCES;
  FrameContainer frames = AcquireTestFrames(3);

  libsync::Completion completed_tx;
  network_device_ifc_.complete_tx_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
          fdf::Arena&, auto&) {
        const auto& results = request->tx;
        ASSERT_EQ(results.size(), 3u);
        EXPECT_EQ(results[0].status, kStatus);
        EXPECT_EQ(results[1].status, kStatus);
        EXPECT_EQ(results[2].status, kStatus);
        completed_tx.Signal();
      };

  test_network_device_.network_device_->CompleteTx(frames, kStatus);
  completed_tx.Wait();
}

TEST_F(NetworkDeviceTestFixture, CompleteRx) {
  size_t storage_size_after_acquire = 0;
  {
    FrameContainer frames = AcquireTestFrames(3);
    storage_size_after_acquire = StorageSize();

    libsync::Completion completed_rx;
    network_device_ifc_.complete_rx_ =
        [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request,
            fdf::Arena&, auto&) {
          auto verifyBuffer = [](const fuchsia_hardware_network_driver::wire::RxBuffer& buffer,
                                 const Frame& frame) {
            ASSERT_EQ(buffer.data.size(), 1u);
            EXPECT_EQ(buffer.meta.port, frame.PortId());
            EXPECT_EQ(buffer.data[0].id, frame.BufferId());
            EXPECT_EQ(buffer.data[0].length, frame.Size());
            EXPECT_EQ(buffer.data[0].offset, frame.Headroom());
          };
          const auto& buffers = request->rx;
          ASSERT_EQ(buffers.size(), 3u);
          verifyBuffer(buffers[0], frames[0]);
          verifyBuffer(buffers[1], frames[1]);
          verifyBuffer(buffers[2], frames[2]);
          completed_rx.Signal();
        };

    test_network_device_.network_device_->CompleteRx(std::move(frames));
    completed_rx.Wait();
  }
  std::lock_guard lock(frame_storage_);
  // The frames should not have been returned to storage
  EXPECT_EQ(frame_storage_.size(), storage_size_after_acquire);
}

TEST_F(NetworkDeviceTestFixture, CompleteRxIgnoreInvalidVmoId) {
  size_t storage_size_after_acquire = 0;
  {
    FrameContainer frames = AcquireTestFrames(3);
    storage_size_after_acquire = StorageSize();

    // Replace the frame with something that has an invalid VMO id, this frame should be discarded
    frames[0] =
        Frame(nullptr, kVmoId + 1, 0, 0, test_network_device_.vmo_addrs_[kVmoId], 256, kPortId);

    libsync::Completion completed_rx;
    network_device_ifc_.complete_rx_ =
        [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request,
            fdf::Arena&, auto&) {
          // Only two results should be present
          const auto& buffers = request->rx;
          ASSERT_EQ(buffers.size(), 2u);
          EXPECT_EQ(buffers[0].data[0].id, frames[1].BufferId());
          EXPECT_EQ(buffers[1].data[0].id, frames[2].BufferId());
          completed_rx.Signal();
        };

    test_network_device_.network_device_->CompleteRx(std::move(frames));
    completed_rx.Wait();
  }
  std::lock_guard lock(frame_storage_);
  // The frame with invalid VMO id should have been returned to storage, only frames that truly
  // completed should be released.
  EXPECT_EQ(frame_storage_.size(), storage_size_after_acquire + 1);
}

TEST_F(NetworkDeviceTestFixture, CompleteRxEmptyFrames) {
  size_t storage_size_after_acquire = 0;
  {
    FrameContainer frames = AcquireTestFrames(3);
    storage_size_after_acquire = StorageSize();
    // Set the size of one frame to 0, this should make it so that it's not reported back to netdev.
    frames[1].SetSize(0);

    libsync::Completion completed_rx;
    network_device_ifc_.complete_rx_ =
        [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request,
            fdf::Arena&, auto&) {
          // All results should be present
          const auto& buffers = request->rx;
          ASSERT_EQ(buffers.size(), 3u);
          EXPECT_EQ(buffers[0].data[0].id, frames[0].BufferId());
          EXPECT_EQ(buffers[1].data[0].id, frames[1].BufferId());
          EXPECT_EQ(buffers[2].data[0].id, frames[2].BufferId());
          completed_rx.Signal();
        };

    test_network_device_.network_device_->CompleteRx(std::move(frames));
    completed_rx.Wait();
  }
  std::lock_guard lock(frame_storage_);
  // The frames should not have been returned to storage.
  EXPECT_EQ(frame_storage_.size(), storage_size_after_acquire);
}

TEST_F(NetworkDeviceTestFixture, GetInfo) {
  // This call should just forward to the base object, the network device can't fill out this info.
  EXPECT_CALL(test_network_device_, NetDevGetInfo(testing::NotNull()));

  fdf::Arena arena('NETD');
  auto info = network_device_client_.sync().buffer(arena)->GetInfo();
  ASSERT_OK(info.status());
}

TEST_F(NetworkDeviceTestFixture, QueueTx) {
  constexpr uint32_t kHeadLength = 42;
  constexpr uint32_t kTailLength = 12;
  constexpr uint32_t kFrameSize = 256;

  // Make sure test parameters are valid, extra head and tail space must fit inside frame size.
  ASSERT_GE(kFrameSize, kHeadLength + kTailLength);

  fdf::Arena arena('NETD');
  fidl::VectorView<fuchsia_hardware_network_driver::wire::TxBuffer> buffers(arena, 3);
  fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion> regions(arena,
                                                                                buffers.size());
  FillTxBuffers(arena, buffers, regions, kFrameSize, kHeadLength, kTailLength);

  libsync::Completion queued_tx;
  EXPECT_CALL(test_network_device_, NetDevQueueTx).WillOnce([&](cpp20::span<Frame> frames) {
    ASSERT_EQ(frames.size(), buffers.size());
    auto verify = [&](const Frame& frame,
                      const fuchsia_hardware_network_driver::wire::TxBuffer& buffer) {
      EXPECT_EQ(frame.VmoId(), buffer.data[0].vmo);
      EXPECT_EQ(frame.BufferId(), buffer.id);
      EXPECT_EQ(frame.PortId(), buffer.meta.port);
      // The queueing logic should shrink the head of the frame so that the head length is before
      // the data pointer. This way the frame will always point to the actual payload.
      EXPECT_EQ(frame.Size(), buffer.data[0].length - kHeadLength - kTailLength);
      EXPECT_EQ(frame.VmoOffset(), buffer.data[0].offset + buffer.head_length);
      EXPECT_EQ(frame.Data(), test_network_device_.vmo_addrs_[frame.VmoId()] +
                                  buffer.data[0].offset + buffer.head_length);
      EXPECT_EQ(frame.Headroom(), buffer.head_length);
    };
    verify(frames[0], buffers[0]);
    verify(frames[1], buffers[1]);
    verify(frames[2], buffers[2]);
    queued_tx.Signal();
  });

  auto queue_tx = network_device_client_.sync().buffer(arena)->QueueTx(buffers);
  ASSERT_OK(queue_tx.status());
  queued_tx.Wait();
}

TEST_F(NetworkDeviceTestFixture, QueueTxWhenStopped) {
  // A stopped device should not queue frames for TX, it should instead reported them as completed
  // to the network device with a status code of ZX_ERR_UNAVAILABLE

  EXPECT_CALL(test_network_device_, NetDevStop)
      .WillOnce([&](NetworkDevice::Callbacks::StopTxn txn) { txn.Reply(); });

  // Stop the device.
  fdf::Arena arena('NETD');
  auto stop = network_device_client_.sync().buffer(arena)->Stop();
  ASSERT_OK(stop.status());

  fidl::VectorView<fuchsia_hardware_network_driver::wire::TxBuffer> buffers(arena, 3);
  fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion> regions(arena,
                                                                                buffers.size());

  FillTxBuffers(arena, buffers, regions, 256, 0, 0);

  EXPECT_CALL(test_network_device_, NetDevQueueTx).Times(0);
  libsync::Completion completed_tx;
  network_device_ifc_.complete_tx_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
          fdf::Arena&, auto&) {
        const auto& results = request->tx;
        EXPECT_EQ(results.size(), buffers.size());
        for (size_t i = 0; i < results.size(); ++i) {
          EXPECT_EQ(results[i].id, buffers[i].id);
          EXPECT_EQ(results[i].status, ZX_ERR_UNAVAILABLE);
        }
        completed_tx.Signal();
      };
  auto queue_tx = network_device_client_.sync().buffer(arena)->QueueTx(buffers);
  ASSERT_OK(queue_tx.status());
  completed_tx.Wait();
}

TEST_F(NetworkDeviceTestFixture, QueueRxSpace) {
  constexpr size_t kFrameSize = 256;

  uint32_t buffer_id = 0;
  size_t vmo_offset = 0;
  auto fill_buffer = [&](fuchsia_hardware_network_driver::wire::RxSpaceBuffer& buffer) {
    buffer = {
        .id = buffer_id++,
        .region = {.vmo = kVmoId, .offset = vmo_offset, .length = kFrameSize},
    };
    vmo_offset += kFrameSize;
  };

  fuchsia_hardware_network_driver::wire::RxSpaceBuffer buffers[3];
  fill_buffer(buffers[0]);
  fill_buffer(buffers[1]);
  fill_buffer(buffers[2]);

  libsync::Completion queue_rx_space_called;
  EXPECT_CALL(test_network_device_, NetDevQueueRxSpace)
      .WillOnce([&](cpp20::span<const fuchsia_hardware_network_driver::wire::RxSpaceBuffer>
                        rx_space_buffers,
                    uint8_t* vmo_addrs[]) {
        ASSERT_EQ(rx_space_buffers.size(), std::size(buffers));
        ASSERT_EQ(rx_space_buffers[0].id, 0u);
        ASSERT_EQ(rx_space_buffers[0].region.vmo, kVmoId);
        ASSERT_EQ(rx_space_buffers[0].region.offset, 0u);
        ASSERT_EQ(rx_space_buffers[0].region.length, kFrameSize);
        ASSERT_EQ(rx_space_buffers[1].id, 1u);
        ASSERT_EQ(rx_space_buffers[1].region.vmo, kVmoId);
        ASSERT_EQ(rx_space_buffers[1].region.offset, kFrameSize);
        ASSERT_EQ(rx_space_buffers[1].region.length, kFrameSize);
        ASSERT_EQ(rx_space_buffers[2].id, 2u);
        ASSERT_EQ(rx_space_buffers[2].region.vmo, kVmoId);
        ASSERT_EQ(rx_space_buffers[2].region.offset, 2u * kFrameSize);
        ASSERT_EQ(rx_space_buffers[2].region.length, kFrameSize);
        queue_rx_space_called.Signal();
      });

  fdf::Arena arena('NETD');
  auto queue_rx = network_device_client_.sync().buffer(arena)->QueueRxSpace({arena, buffers});
  ASSERT_OK(queue_rx.status());
  queue_rx_space_called.Wait();
}

TEST_F(NetworkDeviceTestFixture, Remove) {
  // The first call to Remove should return true to indicate that there was something to remove.
  ASSERT_TRUE(test_network_device_.network_device_->Remove());
  test_network_device_.release_called_.Wait();

  // The second call to Remove should return false to indicate that everything is already remove and
  // that there will be no call to NetDevRelease.
  ASSERT_FALSE(test_network_device_.network_device_->Remove());
}

TEST_F(NetworkDeviceTestFixture, RemoveWhileRemoving) {
  libsync::Completion removed;
  libsync::Completion proceed_with_removal;
  test_network_device_.on_release_ = [&] {
    // Even if we call remove while another remove is in progress there should only ever be one call
    // to NetDevRelease. Which means that removed should never be signaled here.
    EXPECT_FALSE(removed.signaled());
    removed.Signal();
  };

  // Post a task on the dispatcher that will be used for some of the removal steps in NetworkDevice.
  // This will block the removal process from completing until we tell it to.
  async::PostTask(driver_dispatcher_->async_dispatcher(), [&] { proceed_with_removal.Wait(); });

  // Since the first removal won't complete until we tell it to, the second Remove should also
  // return true.
  ASSERT_TRUE(test_network_device_.network_device_->Remove());
  ASSERT_TRUE(test_network_device_.network_device_->Remove());

  proceed_with_removal.Signal();

  removed.Wait();
}

TEST_F(NetworkDeviceTestFixture, RemoveWhilePosting) {
  libsync::Completion removed;
  int num_release_calls = 0;
  test_network_device_.on_release_ = [&] {
    // Even if we call remove while another remove is in progress there should only ever be one call
    // to NetDevRelease. Which means that removed should never be signaled here.
    EXPECT_FALSE(removed.signaled());
    ++num_release_calls;
    removed.Signal();
  };

  // It should be possible to call Remove multiple times and only ever receive one release call.
  while (num_release_calls == 0) {
    test_network_device_.network_device_->Remove();
  }

  removed.Wait();

  driver_test().RunInDriverContext([&](TestDriver& driver) {
    // Run on the driver dispatcher one more time to ensure that all posted tasks have completed.
    // Now we can verify the number of times released was called.
    EXPECT_EQ(num_release_calls, 1);
  });
}

}  // namespace
