// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/watcher.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/hid/boot.h>
#include <lib/stdcompat/string_view.h>
#include <lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h>
#include <lib/zx/fifo.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <vector>

#include <fbl/string.h>
#include <src/devices/lib/client/device_topology.h>
#include <usb/cdc.h>
#include <usb/usb.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/lib/network-device/cpp/network_device_client.h"

namespace usb_virtual_bus {
namespace {

using usb_virtual::BusLauncher;

constexpr const char kManufacturer[] = "Google";
constexpr const char kProduct[] = "CDC Ethernet";
constexpr const char kSerial[] = "ebfd5ad49d2a";
constexpr size_t kEthernetMtu = 1500;

struct DevicePaths {
  std::optional<std::string> path;
  std::string subdir;
  std::string query;
};

zx_status_t WaitForDevice(int dirfd, int event, const char* name, void* cookie) {
  if (std::string_view{name} == ".") {
    return ZX_OK;
  }
  if (event != WATCH_EVENT_ADD_FILE) {
    return ZX_OK;
  }
  fdio_cpp::UnownedFdioCaller caller(dirfd);

  const fit::result response = fdf_topology::GetTopologicalPath(caller.directory(), name);
  if (response.is_error()) {
    return response.error_value();
  }
  std::string_view topological_path = response.value();
  DevicePaths* paths = reinterpret_cast<DevicePaths*>(cookie);
  if (cpp23::contains(topological_path, paths->query)) {
    paths->path = paths->subdir + std::string{name};
    return ZX_ERR_STOP;
  }
  return ZX_OK;
}

class NetworkDeviceInterface {
 public:
  explicit NetworkDeviceInterface(fidl::UnownedClientEnd<fuchsia_io::Directory> directory,
                                  const fbl::String& path, const std::string& session_name)
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    zx::result device = component::ConnectAt<fuchsia_hardware_network::Device>(directory, path);
    ASSERT_OK(device);

    netdevice_client_.emplace(std::move(device.value()), loop_.dispatcher());
    network::client::NetworkDeviceClient& client = netdevice_client_.value();
    {
      std::optional<zx_status_t> result;
      client.OpenSession(session_name, [&result](zx_status_t status) { result = status; });
      RunLoopUntil([&result] { return result.has_value(); });
      ASSERT_OK(result.value());
    }
    ASSERT_TRUE(client.HasSession());
    client.SetRxCallback([&](network::client::NetworkDeviceClient::Buffer buf) {
      rx_queue_.emplace(std::move(buf));
    });
    {
      client.GetPorts(
          [this](zx::result<std::vector<network::client::netdev::wire::PortId>> ports_status) {
            ASSERT_OK(ports_status.status_value());
            std::vector<network::client::netdev::wire::PortId> ports =
                std::move(ports_status.value());
            ASSERT_EQ(ports.size(), 1);
            port_id_ = ports[0];
          });
      RunLoopUntil([this] { return port_id_.has_value(); });
    }
    {
      std::optional<zx_status_t> result;
      client.AttachPort(port_id_.value(), {fuchsia_hardware_network::wire::FrameType::kEthernet},
                        [&result](zx_status_t status) { result = status; });
      RunLoopUntil([&result] { return result.has_value(); });
      ASSERT_OK(result.value());
    }

    network::client::DeviceInfo device_info = client.device_info();
    tx_depth_ = device_info.tx_depth;
    rx_depth_ = device_info.rx_depth;

    {
      bool checked_mtu = false;
      zx::result<std::unique_ptr<network::client::NetworkDeviceClient::StatusWatchHandle>> result =
          client.WatchStatus(
              port_id_.value(),
              [this, &checked_mtu](fuchsia_hardware_network::wire::PortStatus status) {
                if (status.has_mtu()) {
                  mtu_ = status.mtu();
                }
                checked_mtu = true;
              });
      RunLoopUntil([&checked_mtu] { return checked_mtu; });
      ASSERT_TRUE(mtu_.has_value());
    }
  }

  zx_status_t SendData(std::vector<uint8_t>& data) {
    network::client::NetworkDeviceClient::Buffer buffer = netdevice_client_.value().AllocTx();
    if (!buffer.is_valid()) {
      return ZX_ERR_NO_MEMORY;
    }

    // Populate the buffer data and metadata
    buffer.data().SetFrameType(fuchsia_hardware_network::wire::FrameType::kEthernet);
    buffer.data().SetPortId(port_id_.value());
    EXPECT_EQ(buffer.data().Write(data.data(), data.size()), data.size());
    zx_status_t status = buffer.Send();

    // Run the loop to give the Netdevice client an opportunity to send.
    RunLoopUntilIdle();

    return status;
  }

  zx::result<std::vector<uint8_t>> ReceiveData() {
    // Wait for the read callback registered with the Netdevice client to fill the queue.
    RunLoopUntil([this] { return !rx_queue_.empty(); });

    network::client::NetworkDeviceClient::Buffer buffer = std::move(rx_queue_.front());
    rx_queue_.pop();
    if (!buffer.is_valid()) {
      return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
    }
    // Check that the port ID and frame type match what we expect.
    if ((buffer.data().port_id().base != port_id_.value().base) ||
        (buffer.data().port_id().salt != port_id_.value().salt) ||
        (buffer.data().frame_type() != fuchsia_hardware_network::wire::FrameType::kEthernet)) {
      ADD_FAILURE(
          "Frame metadata does not match. Received frame port ID base: %hu \
                  Expected base: %hu \
                  Received frame port ID salt: %hu \
                  Expected salt: %hu \
                  Received frame type: %hu\
                  Expected frame type: %hu",
          buffer.data().port_id().base, port_id_.value().base, buffer.data().port_id().salt,
          buffer.data().port_id().salt, static_cast<uint8_t>(buffer.data().frame_type()),
          static_cast<uint8_t>(fuchsia_hardware_network::wire::FrameType::kEthernet));
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    std::vector<uint8_t> output;
    output.resize(buffer.data().len());
    size_t read = buffer.data().Read(output.data(), output.size());
    if (read != output.size()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok(std::move(output));
  }

  uint32_t tx_depth() const { return tx_depth_; }

  uint32_t rx_depth() const { return rx_depth_; }

  size_t mtu() { return mtu_.value(); }

 private:
  async::Loop loop_;
  std::optional<network::client::NetworkDeviceClient> netdevice_client_;
  // Since the client receives data in a callback, we need to store the frames we receive.
  std::queue<network::client::NetworkDeviceClient::Buffer> rx_queue_;

  std::optional<network::client::netdev::wire::PortId> port_id_;
  uint32_t tx_depth_;
  uint32_t rx_depth_;
  std::optional<size_t> mtu_;

  // TODO(https://fxbug.dev/42065375): remove this hand-rolled implementation (adapted
  // from //sdk/lib/async-loop/testing/real_loop.cc) once `loop_fixture::RealLoop`
  // supports configuration.
  void RunLoopUntil(fit::function<bool()> condition) {
    while (loop_.GetState() == ASYNC_LOOP_RUNNABLE) {
      if (condition()) {
        loop_.ResetQuit();
        return;
      }
      loop_.Run(zx::deadline_after(zx::msec(1)), true);
    }
    loop_.ResetQuit();
  }

  void RunLoopUntilIdle() {
    loop_.RunUntilIdle();
    loop_.ResetQuit();
  }
};

class UsbCdcEcmTest : public zxtest::Test {
 public:
  void SetUp() override {
    auto bus = BusLauncher::Create();
    ASSERT_OK(bus.status_value());
    bus_ = std::move(bus.value());

    ASSERT_NO_FATAL_FAILURE(InitUsbCdcEcm(&peripheral_path_, &host_path_));
  }

  void TearDown() override {
    ASSERT_OK(bus_->ClearPeripheralDeviceFunctions());
    ASSERT_OK(bus_->Disable());
  }

 protected:
  std::optional<BusLauncher> bus_;
  std::string peripheral_path_;
  std::string host_path_;

  void InitUsbCdcEcm(std::string* peripheral_path, std::string* host_path) {
    namespace usb_peripheral = fuchsia_hardware_usb_peripheral;
    using ConfigurationDescriptor =
        ::fidl::VectorView<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>;
    usb_peripheral::wire::DeviceDescriptor device_desc = {};
    device_desc.bcd_usb = htole16(0x0200);
    device_desc.b_device_class = 0;
    device_desc.b_device_sub_class = 0;
    device_desc.b_device_protocol = 0;
    device_desc.b_max_packet_size0 = 64;
    device_desc.bcd_device = htole16(0x0100);
    device_desc.b_num_configurations = 2;

    device_desc.manufacturer = fidl::StringView(kManufacturer);
    device_desc.product = fidl::StringView(kProduct);
    device_desc.serial = fidl::StringView(kSerial);

    device_desc.id_vendor = htole16(0x0BDA);
    device_desc.id_product = htole16(0x8152);

    usb_peripheral::wire::FunctionDescriptor usb_cdc_ecm_function_desc = {
        .interface_class = USB_CLASS_COMM,
        .interface_subclass = USB_CDC_SUBCLASS_ETHERNET,
        .interface_protocol = 0,
    };

    std::vector<usb_peripheral::wire::FunctionDescriptor> function_descs;
    function_descs.push_back(usb_cdc_ecm_function_desc);
    std::vector<ConfigurationDescriptor> config_descs;
    config_descs.emplace_back(
        fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(function_descs));
    config_descs.emplace_back(
        fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(function_descs));

    ASSERT_OK(bus_->SetupPeripheralDevice(std::move(device_desc), std::move(config_descs)));

    const auto wait_for_device = [this](DevicePaths& paths) {
      fbl::unique_fd fd;
      ASSERT_OK(fdio_open3_fd_at(bus_->GetRootFd(), paths.subdir.c_str(),
                                 static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                                 fd.reset_and_get_address()));
      ASSERT_STATUS(fdio_watch_directory(fd.get(), WaitForDevice, ZX_TIME_INFINITE, &paths),
                    ZX_ERR_STOP);
    };
    DevicePaths host_device_paths{
        .subdir = "class/network/",
        .query = "/usb-bus/",
    };
    // Attach to function-001, because it implements usb-cdc-ecm.
    DevicePaths peripheral_device_paths{
        .subdir = "class/network/",
        .query = "/usb-peripheral/function-001",
    };

    wait_for_device(host_device_paths);
    wait_for_device(peripheral_device_paths);

    *host_path = host_device_paths.path.value();
    *peripheral_path = peripheral_device_paths.path.value();
  }
};

void TransmitAndReceive(NetworkDeviceInterface& a, NetworkDeviceInterface& b,
                        const uint32_t fifo_depth) {
  uint8_t fill_data = 0;
  for (size_t i = 0; i < fifo_depth; ++i) {
    std::vector<uint8_t> data;
    for (size_t j = 0; j < kEthernetMtu; ++j) {
      data.push_back(fill_data++);
    }
    ASSERT_OK(a.SendData(data));
  }
  size_t received_bytes = 0;
  uint8_t read_data = 0;
  while (received_bytes < fifo_depth * kEthernetMtu) {
    zx::result<std::vector<uint8_t>> received_data = b.ReceiveData();
    ASSERT_OK(received_data.status_value());
    const std::vector<uint8_t>& data = received_data.value();
    ASSERT_EQ(kEthernetMtu, data.size());
    received_bytes += data.size();
    for (const auto& b : data) {
      ASSERT_EQ(b, read_data++);
    }
  }
}

// TODO(b/316176095): Re-enable test after ensuring it works with DFv2.
TEST_F(UsbCdcEcmTest, DISABLED_TransmitReceive) {
  fdio_cpp::UnownedFdioCaller caller(bus_->GetRootFd());
  NetworkDeviceInterface peripheral(caller.directory(), peripheral_path_,
                                    "usb-cdc-function-peripheral");
  NetworkDeviceInterface host(caller.directory(), host_path_, "usb-cdc-ecm-host");

  ASSERT_EQ(peripheral.mtu(), kEthernetMtu);
  ASSERT_EQ(host.mtu(), kEthernetMtu);

  auto pick_transmit_depth = [](const NetworkDeviceInterface& sender,
                                const NetworkDeviceInterface& receiver) -> uint32_t {
    // NOTE: Netdevice core will double the number of device buffers to account
    // for in-flight buffers with the higher layers. This is encoding knowledge
    // at a distance here, but basically we don't want to send more than the
    // actual underlying device's depth, which makes the transfer racy/fallible.
    //
    // TODO(https://fxbug.dev/42067498): Improve this when we add a signal that we
    // can observe for buffer readiness.
    return std::min(sender.tx_depth(), receiver.rx_depth() / 2);
  };

  TransmitAndReceive(peripheral, host, pick_transmit_depth(peripheral, host));
  TransmitAndReceive(host, peripheral, pick_transmit_depth(host, peripheral));

  ASSERT_NO_FATAL_FAILURE();
}

}  // namespace
}  // namespace usb_virtual_bus
