// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adb.h"

#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/syslog/cpp/macros.h>

namespace adb {

void Adb::ReceiveCallback(
    fidl::WireUnownedResult<fuchsia_hardware_adb::UsbAdbImpl::Receive>& result) {
  if (!result.ok()) {
    // TODO(https://fxbug.dev/42073024): improve the graceful shutdown story in tests and remove
    // this.
    if (result.is_peer_closed() || result.is_canceled()) {
      FX_PLOGS(WARNING, result.status()) << "Connection to underlying UsbAdbImpl failed. Quitting.";
      return;
    }
    FX_PLOGS(ERROR, result.status()) << "Connection to underlying UsbAdbImpl failed. Quitting.";
    return;
  }
  fit::result response = result.value();
  if (response.is_error() && response.error_value() == ZX_ERR_BAD_STATE) {
    FX_PLOGS(ERROR, response.error_value())
        << "Connection to underlying UsbAdbImpl failed. Quitting.";
    return;
  }

  impl_->Receive().Then(fit::bind_member<&Adb::ReceiveCallback>(this));
  if (response.is_error()) {
    FX_PLOGS(ERROR, response.error_value()) << "Failed result";
    return;
  }

  size_t data_left = response.value()->data.size();
  size_t offset = 0;
  size_t copy_len = 0;
  bool complete = false;
  size_t write_len = 0;
  std::unique_ptr<apacket> packet;

  FX_LOGS(DEBUG) << "Start: data left " << data_left << " offset " << offset << " copy_len "
                 << copy_len << " write len " << write_len << " complete " << complete;
  while (data_left) {
    if (packet) {
      FX_LOGS(DEBUG) << "loop running again";
    } else if (pending_packet_) {
      packet = std::move(pending_packet_);
      write_len = copied_len_;
      FX_LOGS(DEBUG) << "Reusing last times packet";
    } else {
      packet = std::make_unique<apacket>();
      write_len = 0;
      FX_LOGS(DEBUG) << "New packet";
    }

    // Header copying
    if (write_len < sizeof(packet->msg)) {
      // header not complete. Let's write header
      size_t header_need = sizeof(packet->msg) - write_len;
      if (data_left >= header_need) {
        complete = true;
        copy_len = header_need;
      } else {
        complete = false;
        copy_len = data_left;
        FX_LOGS(DEBUG) << "Short header";
      }

      memcpy((&packet->msg) + write_len, response->data.data() + offset, copy_len);
      data_left -= copy_len;
      offset += copy_len;
      write_len += copy_len;

      FX_LOGS(DEBUG) << "Loop Hdr: data left " << data_left << " offset " << offset << " copy_len "
                     << copy_len << " write len " << write_len << " complete " << complete;
      if (!complete) {
        continue;
      }
    }

    if (packet->msg.data_length) {
      packet->payload.resize(packet->msg.data_length);
      if (packet->msg.data_length - (write_len - sizeof(packet->msg)) <= data_left) {
        copy_len = packet->msg.data_length - (write_len - sizeof(packet->msg));
        complete = true;
      } else {
        FX_LOGS(DEBUG) << "Short payload";
        complete = false;
        copy_len = data_left;
      }
      memcpy(packet->payload.data() + write_len - sizeof(packet->msg),
             response->data.data() + offset, copy_len);
      data_left -= copy_len;
      offset += copy_len;
      write_len += copy_len;
      FX_LOGS(DEBUG) << "Loop pyld: data left " << data_left << " offset " << offset << " copy_len "
                     << copy_len << " write len " << write_len << " complete " << complete;
    }
    if (complete) {
      transport_.HandleRead(std::move(packet));
      write_len = 0;
    } else {
      continue;
    }
  }

  if (!complete) {
    pending_packet_ = std::move(packet);
    copied_len_ = write_len;
    FX_LOGS(DEBUG) << "Storing incomplete packet " << write_len;
  }

  FX_LOGS(DEBUG) << "End: data left " << data_left << " offset " << offset << " copy_len "
                 << copy_len << " write len " << write_len << " complete " << complete;
}

bool Adb::SendUsbPacket(uint8_t* buf, size_t len) {
  static uint32_t payload_cnt = 0;
  static size_t total_sent = 0;

  payload_cnt++;

  bool success = false;
  auto result = impl_.sync()->QueueTx(fidl::VectorView<uint8_t>::FromExternal(buf, len));
  if (!result.ok() || result->is_error()) {
    FX_LOGS(WARNING) << "Packet " << payload_cnt << " send failed "
                     << (result.ok() ? result->error_value() : ZX_ERR_INTERNAL);
  } else {
    total_sent += len;
    FX_LOGS(DEBUG) << "sent packet " << payload_cnt << " of len " << len << " total " << total_sent;
    success = true;
  }
  FX_LOGS(DEBUG) << "queued packet " << payload_cnt << " " << result->is_ok();
  return success;
}

zx::result<zx::socket> Adb::GetServiceSocket(std::string_view service_name, std::string_view args) {
  auto client_end = service_manager_.CreateDynamicChild(service_name);
  if (client_end.is_error()) {
    FX_LOGS(ERROR) << "Couldn't create/open child for service " << service_name;
    return client_end.take_error();
  }
  ZX_ASSERT(client_end->is_valid());

  zx::socket server, client;
  auto status = zx::socket::create(ZX_SOCKET_STREAM, &server, &client);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Couldn't create sockets " << status;
    return zx::error(status);
  }
  auto result = fidl::WireCall(client_end.value())
                    ->ConnectToService(std::move(server), fidl::StringView::FromExternal(args));
  if (!result.ok() || result->is_error()) {
    status = result.ok() ? result->error_value() : result.status();
    FX_LOGS(ERROR) << "ConnectToService failed " << status;
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

zx_status_t Adb::Init(DeviceConnector* connector) {
  FX_LOGS(DEBUG) << "Only supports 1 adb device. Waiting for device to show up at /svc/"
                 << fuchsia_hardware_adb::Service::Name;
  auto dev = connector->ConnectToFirstDevice();
  if (dev.is_error() || !dev->is_valid()) {
    FX_LOGS(ERROR) << "Could not connect to device at /svc/" << fuchsia_hardware_adb::Service::Name
                   << ": " << dev.error_value();
    return dev.is_error() ? dev.error_value() : ZX_ERR_NOT_CONNECTED;
  }
  device_client_ = std::move(dev.value());

  auto fd_connection =
      std::make_unique<BlockingConnectionAdapter>(std::make_unique<FdConnection>(this));
  transport_.SetConnection(std::move(fd_connection));
  transport_.connection()->Start();

  auto status = StartUsbAdbImpl();
  if (status != ZX_OK) {
    return status;
  }

  status = service_manager_.Init();
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Could not initialize service manager " << status;
    return status;
  }

  FX_LOGS(DEBUG) << "Adb successfully created";
  return ZX_OK;
}

zx_status_t Adb::StartUsbAdbImpl() {
  auto ends = fidl::CreateEndpoints<fuchsia_hardware_adb::UsbAdbImpl>();
  if (!ends.is_ok()) {
    return ends.status_value();
  }

  auto cb = teardown_callback_;
  impl_ = fidl::WireSharedClient<fuchsia_hardware_adb::UsbAdbImpl>(
      std::move(ends->client), dispatcher_, fidl::ObserveTeardown([cb] {
        fit::callback<void()> target = std::move(*cb);
        if (target) {
          target();
        }
      }));
  impl_->Receive().Then(fit::bind_member<&Adb::ReceiveCallback>(this));

  auto result = fidl::WireCall(device_client_)->StartAdb(std::move(ends->server));
  if (result->is_error()) {
    FX_LOGS(ERROR) << "Could not call start for UsbAdbImpl " << result->error_value();
    return result->error_value();
  }
  return ZX_OK;
}

zx::result<std::unique_ptr<Adb>> Adb::Create(async_dispatcher_t* dispatcher) {
  auto adb = std::make_unique<Adb>(dispatcher);
  if (!adb) {
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  // The default device connector that tries to connect to fuchsia_hardware_adb::Device
  // implementations by looking for the first device that appears in
  // /svc/fuchsia.hardware.adb.Service
  class DefaultConnector : public DeviceConnector {
   public:
    explicit DefaultConnector() = default;

    zx::result<fidl::ClientEnd<fuchsia_hardware_adb::Device>> ConnectToFirstDevice() override {
      return component::SyncServiceMemberWatcher<fuchsia_hardware_adb::Service::Adb>()
          .GetNextInstance(false);
    }
  } default_connector;

  auto status = adb->Init(&default_connector);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Could not start adb " << status;
    return zx::error(status);
  }

  return zx::ok(std::move(adb));
}

Adb::~Adb() { *teardown_callback_ = nullptr; }

void Adb::Reset() {
  auto result = fidl::WireCall(device_client_)->StopAdb();
  if (!result.ok()) {
    FX_PLOGS(ERROR, result.status()) << "Failed to stop adb";
  }

  *teardown_callback_ = [this]() mutable { this->StartUsbAdbImpl(); };

  impl_.AsyncTeardown();
}

}  // namespace adb
