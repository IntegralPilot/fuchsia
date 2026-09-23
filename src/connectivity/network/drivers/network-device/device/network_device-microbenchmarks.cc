// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/env.h>
#include <lib/sync/cpp/completion.h>

#include <perftest/perftest.h>

#include "device_interface.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "test_session.h"

#define ZX_ASSERT_OK(status, msg) \
  ZX_ASSERT_MSG((status) == ZX_OK, msg " %s", zx_status_get_string(status))

namespace {

void SyncShutdownBackgroundDispatcher(fdf_dispatcher_t* dispatcher) {
  libsync::Completion shutdown;
  fdf_testing::DriverRuntime::GetInstance()->ShutdownBackgroundDispatcher(
      dispatcher, [&] { shutdown.Signal(); });
  shutdown.Wait();
}

}  // namespace

namespace network {

class FakeDeviceImpl : public fdf::WireServer<netdriver::NetworkPort>,
                       public fdf::WireServer<netdriver::NetworkDeviceImpl> {
 public:
  static constexpr uint16_t kDepth = 256;
  static constexpr uint32_t kMaxBufferLength = 2048;
  static constexpr uint32_t kBufferAlignment = 4096;
  static constexpr uint16_t kPortId = 1;
  static constexpr uint32_t kMtu = 1500;
  static constexpr netdev::wire::FrameType kRxFrameTypes[] = {
      netdev::wire::FrameType::kEthernet,
  };
  static constexpr netdev::wire::FrameTypeSupport kTxFrameTypes[] = {
      {.type = netdev::wire::FrameType::kEthernet},
  };

  explicit FakeDeviceImpl(perftest::RepeatState* state) : perftest_state_(state) {}

  ~FakeDeviceImpl() override {
    SyncShutdownBackgroundDispatcher(impl_dispatcher_->get());
    SyncShutdownBackgroundDispatcher(port_dispatcher_->get());
  }

  void Init(netdriver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
            InitCompleter::Sync& completer) override {
    iface_.Bind(std::move(request->iface));

    auto [client, server] = fdf::Endpoints<netdriver::NetworkPort>::Create();
    fdf::BindServer(port_dispatcher_->get(), std::move(server), this);

    fdf::WireUnownedResult result = iface_.buffer(arena)->AddPort(kPortId, std::move(client));
    ZX_ASSERT_OK(result.status(), "AddPort FIDL error");
    ZX_ASSERT_OK(result->status, "AddPort failed");
    completer.buffer(arena).Reply(ZX_OK);
  }

  void Start(fdf::Arena& arena, StartCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(ZX_OK);
  }

  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override {
    completer.buffer(arena).Reply();
  }

  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<netdriver::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) override {
    auto info = netdriver::wire::DeviceImplInfo::Builder(arena)
                    .tx_depth(kDepth)
                    .rx_depth(kDepth)
                    .rx_threshold(kDepth)
                    .max_buffer_length(kMaxBufferLength)
                    .buffer_alignment(kBufferAlignment)
                    .Build();
    completer.buffer(arena).Reply(info);
  }

  void QueueTx(netdriver::wire::NetworkDeviceImplQueueTxRequest* request, fdf::Arena& arena,
               QueueTxCompleter::Sync& completer) override {
    ZX_ASSERT_MSG(request->buffers.size() <= kDepth, "received %ld tx buffers (depth = %d)",
                  request->buffers.size(), kDepth);
    // NB: This may be called on a thread different than the test thread. To guarantee this doesn't
    // happen concurrently with other perftest actions, the latency test must make sure that no
    // descriptors belong to the device upon each test iteration.
    perftest_state_->NextStep();
    std::array<netdriver::wire::TxResult, kDepth> result;
    auto iter = result.begin();
    for (const auto& buff : request->buffers) {
      *iter++ = {
          .id = buff.id,
          .status = ZX_OK,
      };
    }
    ZX_ASSERT_OK(iface_.buffer(arena)
                     ->CompleteTx(fidl::VectorView<netdriver::wire::TxResult>::FromExternal(
                         result.data(), request->buffers.size()))
                     .status(),
                 "Failed to CompleteTx");
  }

  void QueueRxSpace(netdriver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
                    fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) override {
    ZX_ASSERT_MSG(request->buffers.size() <= kDepth, "received %ld tx buffers (depth = %d)",
                  request->buffers.size(), kDepth);
    // NB: This may be called on a thread different than the test thread. To guarantee this doesn't
    // happen concurrently with other perftest actions, the latency test must make sure that no
    // descriptors belong to the device upon each test iteration.
    perftest_state_->NextStep();
    std::array<netdriver::wire::RxBuffer, kDepth> result;
    std::array<netdriver::wire::RxBufferPart, kDepth> parts;
    auto result_iter = result.begin();
    auto part_iter = parts.begin();
    for (const auto& buff : request->buffers) {
      auto& part = *part_iter++;
      part = {
          .id = buff.id,
          // Any length different than zero will cause the buffer to reach the session, it's
          // irrelevant for the performance test.
          .length = 1024,
      };
      *result_iter++ = {
          .meta =
              {
                  .port = kPortId,
                  .frame_type = netdev::wire::FrameType::kEthernet,
              },
          .data = fidl::VectorView<netdriver::wire::RxBufferPart>::FromExternal(&part, 1)};
    }
    ZX_ASSERT_OK(iface_.buffer(arena)
                     ->CompleteRx(fidl::VectorView<netdriver::wire::RxBuffer>::FromExternal(
                         result.data(), request->buffers.size()))
                     .status(),
                 "Failed to CompleteRx");
  }

  void PrepareVmo(netdriver::wire::NetworkDeviceImplPrepareVmoRequest* request, fdf::Arena& arena,
                  PrepareVmoCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(ZX_OK);
  }

  void ReleaseVmo(netdriver::wire::NetworkDeviceImplReleaseVmoRequest* request, fdf::Arena& arena,
                  ReleaseVmoCompleter::Sync& completer) override {
    completer.buffer(arena).Reply();
  }

  void GetInfo(
      fdf::Arena& arena,
      fdf::WireServer<netdriver::NetworkPort>::GetInfoCompleter::Sync& completer) override {
    auto info = netdev::wire::PortBaseInfo::Builder(arena)
                    .port_class(netdev::wire::PortClass::kEthernet)
                    .rx_types(kRxFrameTypes)
                    .tx_types(kTxFrameTypes)
                    .Build();
    completer.buffer(arena).Reply(info);
  }

  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override {
    auto status = netdev::wire::PortStatus::Builder(arena)
                      .flags(netdev::wire::StatusFlags::kOnline)
                      .mtu(kMtu)
                      .Build();
    completer.buffer(arena).Reply(status);
  }

  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override {}

  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(fdf::ClientEnd<netdriver::MacAddr>{});
  }

  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override {}

  fdf::ClientEnd<netdriver::NetworkDeviceImpl> Bind() {
    auto [client, server] = fdf::Endpoints<netdriver::NetworkDeviceImpl>::Create();
    fdf::BindServer(impl_dispatcher_->get(), std::move(server), this);
    return std::move(client);
  }

 private:
  fdf::UnownedSynchronizedDispatcher impl_dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
  fdf::UnownedSynchronizedDispatcher port_dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
  fdf::WireSyncClient<netdriver::NetworkDeviceIfc> iface_;
  perftest::RepeatState* const perftest_state_;
};

class FakeDeviceImplBinder : public network::NetworkDeviceImplBinder {
 public:
  explicit FakeDeviceImplBinder(FakeDeviceImpl& impl) : impl_(impl) {}

  zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>> Bind() override {
    return zx::ok(impl_.Bind());
  }

 private:
  FakeDeviceImpl& impl_;
};

}  // namespace network

// NB: BaseTestSession is laid out to make the contract clear, we avoid declaring variables with it
// to avoid dynamic dispatch.
class BaseTestSession : public network::testing::TestSession {
 public:
  virtual ~BaseTestSession() = default;
  virtual zx_status_t SendDescriptors(const uint16_t* descriptors, size_t count,
                                      size_t* actual) = 0;
  virtual zx_status_t FetchDescriptors(uint16_t* descriptors, size_t count, size_t* actual) = 0;
  virtual const zx::fifo& test_fifo() = 0;
};

class TxTestSession : public BaseTestSession {
 public:
  zx_status_t SendDescriptors(const uint16_t* descriptors, size_t count, size_t* actual) override {
    return SendTx(descriptors, count, actual);
  }
  zx_status_t FetchDescriptors(uint16_t* descriptors, size_t count, size_t* actual) override {
    return FetchTx(descriptors, count, actual);
  }
  const zx::fifo& test_fifo() override { return tx_fifo(); }
};

class RxTestSession : public BaseTestSession {
 public:
  zx_status_t SendDescriptors(const uint16_t* descriptors, size_t count, size_t* actual) override {
    return SendRx(descriptors, count, actual);
  }
  zx_status_t FetchDescriptors(uint16_t* descriptors, size_t count, size_t* actual) override {
    return FetchRx(descriptors, count, actual);
  }
  const zx::fifo& test_fifo() override { return rx_fifo(); }
};

// LatencyTest measures the round trip latency between a client and a device using an in-process
// fake network device.
//
// The total round trip latency is the time taken for the client to send a batch of packet buffers
// to the device and get them back. This breaks down as follows:
//   - The outbound latency is the time between writing to the FIFO and observing the buffers
//   reaching the device.
//   - The return latency is the time it takes from the device fulfilling those buffers and the
//   client observing them be returned on the FIFO.
//
// The template parameter determines which FIFO and, hence, which path (rx/tx), we're measuring the
// total latency on. Another variation on the test is the number of buffers offered and returned in
// a single batch (limited to the device's FIFO depth).
template <class Session>
bool LatencyTest(perftest::RepeatState* state, const uint16_t buffer_count) {
  ZX_ASSERT_MSG(buffer_count <= network::FakeDeviceImpl::kDepth,
                "can't measure latency with more buffers (%d) than device depth (%d)", buffer_count,
                network::FakeDeviceImpl::kDepth);

  zx::result dispatchers = network::OwnedDeviceInterfaceDispatchers::Create();
  ZX_ASSERT_OK(dispatchers.status_value(), "failed to create dispatchers");

  network::FakeDeviceImpl impl(state);

  zx::result device_status = network::internal::DeviceInterface::Create(
      dispatchers->Unowned(), std::make_unique<network::FakeDeviceImplBinder>(impl));

  ZX_ASSERT_OK(device_status.status_value(), "failed to create device");
  std::unique_ptr device = std::move(device_status.value());

  auto device_endpoints = fidl::Endpoints<network::netdev::Device>::Create();
  ZX_ASSERT_OK(device->Bind(std::move(device_endpoints.server)), "failed to bind to device");

  auto port_endpoints = fidl::Endpoints<network::netdev::Port>::Create();
  ZX_ASSERT_OK(device->BindPort(network::FakeDeviceImpl::kPortId, std::move(port_endpoints.server)),
               "failed to bind port");
  fidl::WireSyncClient port{(std::move(port_endpoints.client))};
  fidl::WireResult port_info_result = port->GetInfo();
  ZX_ASSERT_OK(port_info_result.status(), "failed to get port info");
  const network::netdev::wire::PortInfo& port_info = port_info_result->info;
  ZX_ASSERT_MSG(port_info.has_id(), "port id missing");
  const network::netdev::wire::PortId& port_id = port_info.id();

  Session session;
  fidl::WireSyncClient client{std::move(device_endpoints.client)};
  zx_status_t status =
      session.Open(client, "session", network::netdev::wire::SessionFlags(), buffer_count * 2);
  ZX_ASSERT_OK(status, "failed to open session");
  status = session.AttachPort(port_id, {network::netdev::wire::FrameType::kEthernet});
  ZX_ASSERT_OK(status, "failed to attach port");

  std::array<uint16_t, network::FakeDeviceImpl::kDepth> write_descriptors, returned_descriptors;
  for (uint16_t i = 0; i < buffer_count; i++) {
    buffer_descriptor_t& descriptor = session.ResetDescriptor(i);
    // Tx tests need to set the port id here.
    descriptor.port_id = {
        .base = port_id.base,
        .salt = port_id.salt,
    };
    write_descriptors[i] = i;
  }

  state->DeclareStep("outbound");
  state->DeclareStep("return");
  while (state->KeepRunning()) {
    size_t actual;
    status = session.SendDescriptors(&*write_descriptors.begin(), buffer_count, &actual);
    ZX_ASSERT_OK(status, "failed to send descriptors");
    ZX_ASSERT_MSG(actual == buffer_count, "partial FIFO write %ld/%d", actual, buffer_count);

    status = session.test_fifo().wait_one(ZX_FIFO_READABLE, zx::time::infinite(), nullptr);
    ZX_ASSERT_OK(status, "wait FIFO readable");
    status = session.FetchDescriptors(&*returned_descriptors.begin(), buffer_count, &actual);
    ZX_ASSERT_OK(status, "failed to fetch descriptors");
    // Guarantee that all descriptors we sent come back to us, so the device can't be making any
    // work in its background threads.
    ZX_ASSERT_MSG(actual == buffer_count, "unexpected partial FIFO batch read %ld/%d", actual,
                  buffer_count);
  }

  sync_completion_t completion;
  device->Teardown([&completion]() { sync_completion_signal(&completion); });
  status = sync_completion_wait(&completion, zx::duration::infinite().get());
  ZX_ASSERT_OK(status, "sync_completion_wait(_, _) failed ");
  dispatchers->ShutdownSync();
  return true;
}

void RegisterTests() {
  constexpr uint16_t kBatchSizes[] = {1, 8, 16, 64, 256};
  for (auto& batch_size : kBatchSizes) {
    perftest::RegisterTest(fxl::StringPrintf("Latency/Rx/%d", batch_size).c_str(),
                           LatencyTest<RxTestSession>, batch_size);
    perftest::RegisterTest(fxl::StringPrintf("Latency/Tx/%d", batch_size).c_str(),
                           LatencyTest<TxTestSession>, batch_size);
  }
}
PERFTEST_CTOR(RegisterTests)

int main(int argc, char** argv) {
  fdf_testing::DriverRuntime runtime;

  constexpr char kTestSuiteName[] = "fuchsia.network.device";
  return perftest::PerfTestMain(argc, argv, kTestSuiteName);
}
