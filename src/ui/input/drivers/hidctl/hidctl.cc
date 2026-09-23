// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hidctl.h"

#include <fidl/fuchsia.hardware.hidctl/cpp/wire.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <stdio.h>
#include <string.h>
#include <zircon/compiler.h>

#include <memory>
#include <utility>
#include <cinttypes>

#include <fbl/array.h>
#include <fbl/auto_lock.h>
#include <pretty/hexdump.h>

namespace hidctl {

namespace fhidbus = fuchsia_hardware_hidbus;

zx_status_t HidCtl::Create(void* ctx, zx_device_t* parent) {
  auto dev = std::unique_ptr<HidCtl>(new HidCtl(parent));
  zx_status_t status = dev->DdkAdd(ddk::DeviceAddArgs("hidctl").set_flags(DEVICE_ADD_NON_BINDABLE));
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: could not add device: %d", __func__, status);
  } else {
    // devmgr owns the memory now
    [[maybe_unused]] auto* ptr = dev.release();
  }
  return status;
}

void HidCtl::MakeHidDevice(MakeHidDeviceRequestView request,
                           MakeHidDeviceCompleter::Sync& completer) {
  // Create the sockets for Sending/Recieving fake HID reports.
  zx::socket local, remote;
  zx_status_t status = zx::socket::create(ZX_SOCKET_DATAGRAM, &local, &remote);
  if (status != ZX_OK) {
    completer.Close(status);
    return;
  }

  // Create the fake HID device.
  uint8_t* report_desc_data = new uint8_t[request->rpt_desc.size()];
  memcpy(report_desc_data, request->rpt_desc.data(), request->rpt_desc.size());
  fbl::Array<const uint8_t> report_desc(report_desc_data, request->rpt_desc.size());
  auto hiddev = std::make_unique<hidctl::HidDevice>(zxdev(), request->config,
                                                    std::move(report_desc), std::move(local));

  auto client_end = hiddev->ServeOutgoing();

  std::array offers = {
      fhidbus::Service::Name,
  };
  status = hiddev->DdkAdd(ddk::DeviceAddArgs("hidctl-dev")
                              .set_fidl_service_offers(offers)
                              .set_outgoing_dir(client_end->TakeChannel()));
  if (status != ZX_OK) {
    zxlogf(ERROR, "hidctl: could not add hid device: %d", status);
    completer.Close(status);
    return;
  }
  // The device thread will be created in DdkInit.

  zxlogf(INFO, "hidctl: created hid device");
  // devmgr owns the memory until release is called
  [[maybe_unused]] auto ptr = hiddev.release();

  completer.Reply(std::move(remote));
}

HidCtl::HidCtl(zx_device_t* device) : DeviceType(device) {}

void HidCtl::DdkRelease() { delete this; }

HidDevice::HidDevice(zx_device_t* device, const fuchsia_hardware_hidctl::wire::HidCtlConfig& config,
                     fbl::Array<const uint8_t> report_desc, zx::socket data)
    : ddk::Device<HidDevice, ddk::Initializable, ddk::Unbindable>(device),
      outgoing_(fdf::Dispatcher::GetCurrent()->async_dispatcher()),
      boot_protocol_(config.boot_device
                         ? static_cast<fhidbus::wire::HidBootProtocol>(config.dev_class)
                         : fhidbus::wire::HidBootProtocol::kNone),
      report_desc_(std::move(report_desc)),
      data_(std::move(data)) {
  ZX_DEBUG_ASSERT(data_.is_valid());
}

void HidDevice::DdkInit(ddk::InitTxn txn) {
  wait_handler_.set_object(data_.get());
  wait_handler_.set_trigger(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED);
  txn.Reply(wait_handler_.Begin(fdf::Dispatcher::GetCurrent()->async_dispatcher()));
}

void HidDevice::DdkRelease() {
  zxlogf(DEBUG, "hidctl: DdkRelease");
  delete this;
}

void HidDevice::DdkUnbind(ddk::UnbindTxn txn) {
  zxlogf(DEBUG, "hidctl: DdkUnbind");
  fbl::AutoLock lock(&lock_);
  if (data_.is_valid()) {
    // Prevent further writes to the socket
    zx_status_t status = data_.set_disposition(0, ZX_SOCKET_DISPOSITION_WRITE_DISABLED);
    ZX_DEBUG_ASSERT(status == ZX_OK);
    // Signal the thread to shutdown
    wait_handler_.Cancel();
  }

  txn.Reply();
}

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> HidDevice::ServeOutgoing() {
  zx::result<> result = outgoing_.AddService<fhidbus::Service>(fhidbus::Service::InstanceHandler({
      .device =
          [this](fidl::ServerEnd<fhidbus::Hidbus> server_end) {
            if (binding_) {
              server_end.Close(ZX_ERR_ALREADY_BOUND);
              return;
            }
            binding_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                             std::move(server_end), this, [this](fidl::UnbindInfo info) {
                               Stop();
                               binding_.reset();
                             });
          },
  }));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to add Hidbus protocol: %s", result.status_string());
    return result.take_error();
  }
  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  result = outgoing_.Serve(std::move(server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to service the outgoing directory");
    return result.take_error();
  }

  return zx::ok(std::move(client));
}

void HidDevice::Query(QueryCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: query");

  fidl::Arena<> arena;
  completer.ReplySuccess(fhidbus::wire::HidInfo::Builder(arena)
                             .dev_num(0)
                             .boot_protocol(boot_protocol_)
                             .vendor_id(0)
                             .product_id(0)
                             .version(0)
                             .polling_rate(0)
                             .Build());
}

void HidDevice::Start(StartCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: start");

  if (started_) {
    zxlogf(ERROR, "Already started");
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }

  started_ = true;
  completer.ReplySuccess();
}

void HidDevice::Stop(StopCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: stop");

  Stop();
}

void HidDevice::Stop() { started_ = false; }

void HidDevice::GetDescriptor(fhidbus::wire::HidbusGetDescriptorRequest* request,
                              GetDescriptorCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: get descriptor %" PRIu8, static_cast<uint8_t>(request->desc_type));

  if (request->desc_type != fhidbus::wire::HidDescriptorType::kReport) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(
      const_cast<uint8_t*>(report_desc_.data()), report_desc_.size()));
}

void HidDevice::SetDescriptor(fhidbus::wire::HidbusSetDescriptorRequest* request,
                              SetDescriptorCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void HidDevice::GetReport(fhidbus::wire::HidbusGetReportRequest* request,
                          GetReportCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: get report type=%" PRIu8 " id=%" PRIu8,
         static_cast<uint8_t>(request->rpt_type), request->rpt_id);

  // TODO: send get report message over socket
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void HidDevice::SetReport(fhidbus::wire::HidbusSetReportRequest* request,
                          SetReportCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: set report type=%" PRIu8 " id=%" PRIu8,
         static_cast<uint8_t>(request->rpt_type), request->rpt_id);

  // TODO: send set report message over socket
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void HidDevice::GetIdle(fhidbus::wire::HidbusGetIdleRequest* request,
                        GetIdleCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: get idle");

  // TODO: send get idle message over socket
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void HidDevice::SetIdle(fhidbus::wire::HidbusSetIdleRequest* request,
                        SetIdleCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: set idle");

  // TODO: send set idle message over socket
  completer.ReplySuccess();
}

void HidDevice::GetProtocol(GetProtocolCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: get protocol");

  // TODO: send get protocol message over socket
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void HidDevice::SetProtocol(fhidbus::wire::HidbusSetProtocolRequest* request,
                            SetProtocolCompleter::Sync& completer) {
  zxlogf(DEBUG, "hidctl: set protocol");

  // TODO: send set protocol message over socket
  completer.ReplySuccess();
}

void HidDevice::HandleData(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                           zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    zxlogf(ERROR, "hidctl: error waiting on data: %d", status);
    DdkAsyncRemove();
    return;
  }

  if (signal->observed & ZX_SOCKET_READABLE) {
    status = Recv(buf_.data(), mtu_);
    if (status != ZX_OK) {
      DdkAsyncRemove();
      return;
    }
  }
  if (signal->observed & ZX_SOCKET_PEER_CLOSED) {
    zxlogf(DEBUG, "hidctl: socket closed (peer)");
    DdkAsyncRemove();
    return;
  }

  wait_handler_.Begin(dispatcher);
}

zx_status_t HidDevice::Recv(uint8_t* buffer, uint32_t capacity) {
  size_t actual = 0;
  zx_status_t status = ZX_OK;
  // Read all the datagrams out of the socket.
  while (status == ZX_OK) {
    status = data_.read(0u, buffer, capacity, &actual);
    if (status == ZX_ERR_SHOULD_WAIT || status == ZX_ERR_PEER_CLOSED) {
      break;
    }
    if (status != ZX_OK) {
      zxlogf(ERROR, "hidctl: error reading data: %d", status);
      return status;
    }

    fbl::AutoLock lock(&lock_);
    if (unlikely(zxlog_level_enabled(DEBUG))) {
      zxlogf(DEBUG, "hidctl: received %zu bytes", actual);
      hexdump8_ex(buffer, actual, 0);
    }
    if (started_ && binding_) {
      fidl::Arena arena;
      auto result = fidl::WireSendEvent(*binding_)->OnReportReceived(
          fhidbus::wire::Report::Builder(arena)
              .buf(fidl::VectorView<uint8_t>::FromExternal(buffer, actual))
              .timestamp(zx_clock_get_monotonic())
              .Build());
      if (!result.ok()) {
        zxlogf(ERROR, "OnReportReceived failed %s", result.error().FormatDescription().c_str());
      }
    }
  }
  return ZX_OK;
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = HidCtl::Create;
  return ops;
}();

}  // namespace hidctl

ZIRCON_DRIVER(hidctl, hidctl::driver_ops, "zircon", "0.1");
