// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bt_transport_usb.h"

#include <assert.h>
#include <fuchsia/hardware/usb/c/banjo.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/completion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls/port.h>
#include <zircon/types.h>

#include <bind/fuchsia/bluetooth/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <fbl/auto_lock.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

#include "src/lib/listnode/listnode.h"

constexpr int EVENT_REQ_COUNT = 8;
constexpr int ACL_READ_REQ_COUNT = 8;
constexpr int ACL_WRITE_REQ_COUNT = 10;
constexpr int SCO_READ_REQ_COUNT = 8;

// The maximum HCI ACL frame size used for data transactions
constexpr uint64_t ACL_MAX_FRAME_SIZE = 1028;  // (1024 + 4 bytes for the ACL header)

namespace bt_transport_usb {
namespace fhbt = fuchsia_hardware_bluetooth;
namespace {

// Endpoints: interrupt, bulk in, bulk out
constexpr uint8_t kNumInterface0Endpoints = 3;
// Endpoints: isoc in, isoc out
constexpr uint8_t kNumInterface1Endpoints = 2;

constexpr uint8_t kIsocInterfaceNum = 1;
constexpr uint8_t kIsocAltSettingInactive = 0;

constexpr uint8_t kScoWriteReqCount = 8;
constexpr uint16_t kScoMaxFrameSize = 255 + 3;  // payload + 3 bytes header

struct HciEventHeader {
  uint8_t event_code;
  uint8_t parameter_total_size;
} __PACKED;

}  // namespace

// fhbt::ScoConnection overrides.
ScoConnectionServer::ScoConnectionServer(
    async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_hardware_bluetooth::ScoConnection> server_end, SendHandler send_handler,
    StopHandler stop_handler, fit::function<void(fidl::UnbindInfo)> close_handler)
    : send_handler_(std::move(send_handler)),
      stop_handler_(std::move(stop_handler)),
      binding_(dispatcher, std::move(server_end), this, std::move(close_handler)) {}

void ScoConnectionServer::Send(SendRequest& request, SendCompleter::Sync& completer) {
  send_handler_(request.packet(), completer);
}

void ScoConnectionServer::AckReceive(AckReceiveCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/349616746): Implement flow control based on AckReceive.
}
void ScoConnectionServer::Stop(StopCompleter::Sync& completer) {
  binding_.Close(ZX_OK);
  stop_handler_();
}

void ScoConnectionServer::handle_unknown_method(
    ::fidl::UnknownMethodMetadata<fhbt::ScoConnection> metadata,
    ::fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "Unknown method in ScoConnection protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

Device::Device(zx_device_t* parent, async_dispatcher_t* dispatcher)
    : DeviceType(parent),
      dispatcher_(dispatcher),
      sco_reassembler_(
          /*length_param_index=*/2, /*header_size=*/3,
          [this](cpp20::span<const uint8_t> pkt) { OnScoReassemblerPacketLocked(pkt); }) {}

zx_status_t Device::Create(void* /*ctx*/, zx_device_t* parent) {
  return Create(parent, /*dispatcher=*/nullptr);
}

zx_status_t Device::Create(zx_device_t* parent, async_dispatcher_t* dispatcher) {
  std::unique_ptr<Device> dev = std::make_unique<Device>(parent, dispatcher);

  zx_status_t bind_status = dev->Bind();
  if (bind_status != ZX_OK) {
    zxlogf(ERROR, "bt-transport-usb: failed to bind: %s", zx_status_get_string(bind_status));
    return bind_status;
  }

  // Driver Manager is now in charge of the device.
  // Memory will be explicitly freed in DdkUnbind().
  [[maybe_unused]] Device* unused = dev.release();
  return ZX_OK;
}

zx_status_t Device::Bind() {
  zxlogf(DEBUG, "%s", __FUNCTION__);

  usb_protocol_t usb;
  zx_status_t status = device_get_protocol(parent(), ZX_PROTOCOL_USB, &usb);
  if (status != ZX_OK) {
    zxlogf(ERROR, "bt-transport-usb: get protocol failed: %s", zx_status_get_string(status));
    return status;
  }
  memcpy(&usb_, &usb, sizeof(usb));

  // This driver takes on the responsibility of servicing the two contained interfaces of the
  // configuration.
  uint8_t configuration = usb_get_configuration(&usb);

  size_t config_desc_length = 0;
  status = usb_get_configuration_descriptor_length(&usb, configuration, &config_desc_length);
  if (status != ZX_OK) {
    zxlogf(ERROR, "get config descriptor length failed: %s", zx_status_get_string(status));
    return status;
  }
  std::vector<uint8_t> desc_bytes(config_desc_length, 0);

  size_t out_desc_actual = 0;
  status = usb_get_configuration_descriptor(&usb, configuration, desc_bytes.data(),
                                            desc_bytes.size(), &out_desc_actual);
  if (status != ZX_OK) {
    zxlogf(ERROR, "get config descriptor length failed: %s", zx_status_get_string(status));
    return status;
  }
  if (out_desc_actual != config_desc_length) {
    zxlogf(ERROR, "get config descriptor length failed: out_desc_actual != config_desc_length");
    return status;
  }

  if (desc_bytes.size() < sizeof(usb_configuration_descriptor_t)) {
    zxlogf(ERROR, "Malformed USB descriptor detected!");
    return ZX_ERR_NOT_SUPPORTED;
  }
  usb_configuration_descriptor_t* config =
      reinterpret_cast<usb_configuration_descriptor_t*>(desc_bytes.data());

  // Get the configuration descriptor, which contains interface descriptors.
  usb_desc_iter_t config_desc_iter;
  status = usb_desc_iter_init_unowned(config, le16toh(config->w_total_length), &config_desc_iter);
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to get usb configuration descriptor iter: %s",
           zx_status_get_string(status));
    return status;
  }

  // Interface 0 should contain the interrupt, bulk in, and bulk out endpoints.
  // See Core Spec v5.3, Vol 4, Part B, Sec 2.1.1.
  usb_interface_descriptor_t* intf =
      usb_desc_iter_next_interface(&config_desc_iter, /*skip_alt=*/true);

  if (!intf) {
    zxlogf(DEBUG, "%s: failed to get usb interface 0", __FUNCTION__);
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (intf->b_num_endpoints != kNumInterface0Endpoints) {
    zxlogf(DEBUG, "%s: interface has %hhu endpoints, expected %hhu", __FUNCTION__,
           intf->b_num_endpoints, kNumInterface0Endpoints);
    return ZX_ERR_NOT_SUPPORTED;
  }

  usb_endpoint_descriptor_t* endp = usb_desc_iter_next_endpoint(&config_desc_iter);
  while (endp) {
    if (usb_ep_direction(endp) == USB_ENDPOINT_OUT) {
      if (usb_ep_type(endp) == fuchsia_hardware_usb_descriptor::EndpointType::kBulk) {
        bulk_out_endp_desc_ = *endp;
      }
    } else {
      ZX_ASSERT(usb_ep_direction(endp) == USB_ENDPOINT_IN);
      if (usb_ep_type(endp) == fuchsia_hardware_usb_descriptor::EndpointType::kBulk) {
        bulk_in_endp_desc_ = *endp;
      } else if (usb_ep_type(endp) == fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt) {
        intr_endp_desc_ = *endp;
      }
    }
    endp = usb_desc_iter_next_endpoint(&config_desc_iter);
  }

  if (!bulk_in_endp_desc_ || !bulk_out_endp_desc_ || !intr_endp_desc_) {
    zxlogf(ERROR, "bt-transport-usb: bind could not find bulk in/out and interrupt endpoints");
    return ZX_ERR_NOT_SUPPORTED;
  }

  usb_enable_endpoint(&usb, &bulk_in_endp_desc_.value(), /*ss_com_desc=*/nullptr, /*enable=*/true);
  usb_enable_endpoint(&usb, &bulk_out_endp_desc_.value(), /*ss_com_desc=*/nullptr, /*enable=*/true);
  usb_enable_endpoint(&usb, &intr_endp_desc_.value(), /*ss_com_desc=*/nullptr, /*enable=*/true);
  uint16_t intr_max_packet = usb_ep_max_packet(intr_endp_desc_);

  status = usb_set_interface(&usb, intf->b_interface_number, /*alt_setting=*/0);
  if (status != ZX_OK) {
    zxlogf(ERROR, "usb set interface %hhu failed: %s", intf->b_interface_number,
           zx_status_get_string(status));
    return status;
  }

  // If SCO is supported, interface 1 should contain the isoc in and isoc out endpoints. See Core
  // Spec v5.3, Vol 4, Part B, Sec 2.1.1.
  ReadIsocInterfaces(&config_desc_iter);
  if (!isoc_endp_descriptors_.empty()) {
    status = usb_set_interface(&usb, /*interface_number=*/1, kIsocAltSettingInactive);
    if (status != ZX_OK) {
      zxlogf(ERROR, "usb set interface 1 failed: %s", zx_status_get_string(status));
      return status;
    }
  } else {
    zxlogf(INFO, "SCO is not supported");
  }

  list_initialize(&free_event_reqs_);
  list_initialize(&free_acl_read_reqs_);
  list_initialize(&free_acl_write_reqs_);
  list_initialize(&free_sco_read_reqs_);
  list_initialize(&free_sco_write_reqs_);

  parent_req_size_ = usb_get_request_size(&usb);
  size_t req_size = parent_req_size_ + sizeof(usb_req_internal_t) + sizeof(void*);
  status = AllocBtUsbPackets(EVENT_REQ_COUNT, intr_max_packet, intr_endp_desc_->b_endpoint_address,
                             req_size, &free_event_reqs_);
  if (status != ZX_OK) {
    OnBindFailure(status, "event USB request allocation failure");
    return status;
  }
  status =
      AllocBtUsbPackets(ACL_READ_REQ_COUNT, ACL_MAX_FRAME_SIZE,
                        bulk_in_endp_desc_->b_endpoint_address, req_size, &free_acl_read_reqs_);
  if (status != ZX_OK) {
    OnBindFailure(status, "ACL read USB request allocation failure");
    return status;
  }
  status =
      AllocBtUsbPackets(ACL_WRITE_REQ_COUNT, ACL_MAX_FRAME_SIZE,
                        bulk_out_endp_desc_->b_endpoint_address, req_size, &free_acl_write_reqs_);
  if (status != ZX_OK) {
    OnBindFailure(status, "ACL write USB request allocation failure");
    return status;
  }

  if (!isoc_endp_descriptors_.empty()) {
    // We assume that the endpoint addresses are the same across alternate settings, so we only need
    // to allocate packets once.
    const uint8_t isoc_out_endp_address = isoc_endp_descriptors_[0].out.b_endpoint_address;
    status = AllocBtUsbPackets(kScoWriteReqCount, kScoMaxFrameSize, isoc_out_endp_address, req_size,
                               &free_sco_write_reqs_);
    if (status != ZX_OK) {
      OnBindFailure(status, "SCO write USB request allocation failure");
      return status;
    }

    const uint8_t isoc_in_endp_address = isoc_endp_descriptors_[0].in.b_endpoint_address;
    status = AllocBtUsbPackets(SCO_READ_REQ_COUNT, kScoMaxFrameSize, isoc_in_endp_address, req_size,
                               &free_sco_read_reqs_);
    if (status != ZX_OK) {
      OnBindFailure(status, "SCO read USB request allocation failure");
      return status;
    }
  }

  {
    std::lock_guard lock(mutex_);
    QueueInterruptRequestsLocked();
    QueueAclReadRequestsLocked();
  }
  // We don't need to queue SCO packets here because they will be queued when the alt setting is
  // changed.

  // Copy the PID and VID from the underlying BT so that it can be filtered on
  // for HCI drivers
  usb_device_descriptor_t dev_desc;
  usb_get_device_descriptor(&usb, &dev_desc);
  zx_device_str_prop_t props[] = {
      ddk::MakeStrProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_bluetooth::BIND_PROTOCOL_TRANSPORT),
      ddk::MakeStrProperty(bind_fuchsia::USB_VID, static_cast<uint32_t>(dev_desc.id_vendor)),
      ddk::MakeStrProperty(bind_fuchsia::USB_PID, static_cast<uint32_t>(dev_desc.id_product)),
  };
  zxlogf(DEBUG, "bt-transport-usb: vendor id = %hu, product id = %hu", dev_desc.id_vendor,
         dev_desc.id_product);

  // Use default driver dispatcher in production. In tests, use the test dispatcher provided in the
  // constructor.
  if (!dispatcher_) {
    dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  }

  // Serve HciTransport protocol in the outgoing directory and add the directory to the child
  // device.
  outgoing_ = component::OutgoingDirectory(dispatcher_);

  auto snoop_handler = [this](fidl::ServerEnd<fhbt::Snoop> server_end) mutable {
    if (snoop_server_.has_value()) {
      zxlogf(ERROR, "Snoop already active");
      return;
    }
    snoop_server_.emplace(dispatcher_, std::move(server_end), this, [this](fidl::UnbindInfo) {
      snoop_server_->Close(ZX_OK);
      snoop_server_.reset();
    });
  };

  fuchsia_hardware_bluetooth::HciService::InstanceHandler handler({
      .hci_transport =
          hci_transport_binding_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
      .snoop = std::move(snoop_handler),
  });
  auto result = outgoing_->AddService<fuchsia_hardware_bluetooth::HciService>(std::move(handler));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to add service to the outgoing directory");
    return result.status_value();
  }

  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return endpoints.status_value();
  }

  result = outgoing_->Serve(std::move(endpoints->server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to serve the outgoing directory");
    return result.status_value();
  }

  std::array offers = {
      fuchsia_hardware_bluetooth::HciService::Name,
  };

  status = DdkAdd(ddk::DeviceAddArgs("bt-transport-usb")
                      .set_str_props(props)
                      .set_proto_id(ZX_PROTOCOL_BT_TRANSPORT)
                      .set_fidl_service_offers(offers)
                      .set_outgoing_dir(endpoints->client.TakeChannel()));
  if (status != ZX_OK) {
    OnBindFailure(status, "DdkAdd");
    return status;
  }

  return ZX_OK;
}

void Device::ConnectHciTransport(
    fidl::ServerEnd<fuchsia_hardware_bluetooth::HciTransport> server_end) {
  hci_transport_binding_.AddBinding(dispatcher_, std::move(server_end), this,
                                    fidl::kIgnoreBindingClosure);
}

void Device::ConnectSnoop(fidl::ServerEnd<fuchsia_hardware_bluetooth::Snoop> server_end) {
  snoop_server_.emplace(dispatcher_, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

zx_status_t Device::DdkGetProtocol(uint32_t proto_id, void* out) {
  if (proto_id == ZX_PROTOCOL_USB) {
    // Pass this on for drivers to load firmware / initialize
    return device_get_protocol(parent(), proto_id, out);
  }

  return ZX_OK;
}

void Device::DdkUnbind(ddk::UnbindTxn txn) {
  zxlogf(DEBUG, "%s", __FUNCTION__);

  // Do cleanup on the work thread to avoid blocking the driver manager thread.
  async::PostTask(dispatcher_, [this, unbind_txn = std::move(txn)]() mutable {
    if (loop_) {
      zxlogf(DEBUG, "quitting read thread");
      loop_->Quit();
    }

    const uint8_t isoc_alt_setting = [&]() {
      std::lock_guard lock(mutex_);

      {
        std::lock_guard lock(pending_request_lock_);
        unbound_ = true;
      }

      return isoc_alt_setting_;
    }();

    zxlogf(DEBUG, "DdkUnbind: canceling all requests");
    // usb_cancel_all synchronously cancels all requests.
    zx_status_t status = usb_cancel_all(&usb_, bulk_out_endp_desc_->b_endpoint_address);
    if (status != ZX_OK) {
      zxlogf(ERROR, "canceling bulk out requests failed with status: %s",
             zx_status_get_string(status));
    }

    status = usb_enable_endpoint(&usb_, &bulk_out_endp_desc_.value(),
                                 /*ss_com_desc=*/nullptr, /*enable=*/false);
    if (status != ZX_OK) {
      zxlogf(ERROR, "disabling bulk out endpoint failed with status: %s",
             zx_status_get_string(status));
    }

    status = usb_cancel_all(&usb_, bulk_in_endp_desc_->b_endpoint_address);
    if (status != ZX_OK) {
      zxlogf(ERROR, "canceling bulk in requests failed with status: %s",
             zx_status_get_string(status));
    }

    status = usb_enable_endpoint(&usb_, &bulk_in_endp_desc_.value(),
                                 /*ss_com_desc=*/nullptr, /*enable=*/false);
    if (status != ZX_OK) {
      zxlogf(ERROR, "disabling bulk in endpoint failed with status: %s",
             zx_status_get_string(status));
    }

    status = usb_cancel_all(&usb_, intr_endp_desc_->b_endpoint_address);
    if (status != ZX_OK) {
      zxlogf(ERROR, "canceling interrupt requests failed with status: %s",
             zx_status_get_string(status));
    }

    status = usb_enable_endpoint(&usb_, &intr_endp_desc_.value(),
                                 /*ss_com_desc=*/nullptr, /*enable=*/false);
    if (status != ZX_OK) {
      zxlogf(ERROR, "disabling interrupt endpoint failed with status: %s",
             zx_status_get_string(status));
    }

    // Disable ISOC endpoints & cancel requests.
    if (isoc_alt_setting != kIsocAltSettingInactive) {
      status =
          usb_cancel_all(&usb_, isoc_endp_descriptors_[isoc_alt_setting].in.b_endpoint_address);
      if (status != ZX_OK) {
        zxlogf(ERROR, "canceling isoc in requests failed with status: %s",
               zx_status_get_string(status));
      }

      status = usb_enable_endpoint(&usb_, &isoc_endp_descriptors_[isoc_alt_setting].in,
                                   /*ss_com_desc=*/nullptr, /*enable=*/false);
      if (status != ZX_OK) {
        zxlogf(ERROR, "disabling isoc in endpoint failed with status: %s",
               zx_status_get_string(status));
      }

      status =
          usb_cancel_all(&usb_, isoc_endp_descriptors_[isoc_alt_setting].out.b_endpoint_address);
      if (status != ZX_OK) {
        zxlogf(ERROR, "canceling isoc out requests failed with status: %s",
               zx_status_get_string(status));
      }

      status = usb_enable_endpoint(&usb_, &isoc_endp_descriptors_[isoc_alt_setting].out,
                                   /*ss_com_desc=*/nullptr, /*enable=*/false);
      if (status != ZX_OK) {
        zxlogf(ERROR, "disabling isoc out endpoint failed with status: %s",
               zx_status_get_string(status));
      }
    }
    zxlogf(DEBUG, "all pending requests canceled & endpoints disabled");

    unbind_txn.Reply();
  });
}

void Device::DdkRelease() {
  zxlogf(DEBUG, "%s", __FUNCTION__);

  if (loop_) {
    loop_->JoinThreads();
    zxlogf(DEBUG, "read thread joined");
  }

  {
    std::lock_guard lock(mutex_);
    const auto reqs_lists = {&free_event_reqs_, &free_acl_read_reqs_, &free_acl_write_reqs_,
                             &free_sco_read_reqs_, &free_sco_write_reqs_};
    for (list_node_t* list : reqs_lists) {
      usb_request_t* req;
      while ((req = usb_req_list_remove_head(list, parent_req_size_)) != nullptr) {
        InstrumentedRequestRelease(req);
      }
    }
  }

  // Wait for all the requests in the pipeline to asynchronously fail.
  // Either the completion routine or the submitter should free the requests.
  // It shouldn't be possible to have any "stray" requests that aren't in-flight at this point,
  // so this is guaranteed to complete.
  zxlogf(DEBUG, "%s: waiting for all requests to be freed before releasing", __FUNCTION__);
  sync_completion_wait(&requests_freed_completion_, ZX_TIME_INFINITE);
  zxlogf(DEBUG, "%s: all requests freed", __FUNCTION__);

  sco_connection_server_.reset();

  hci_transport_binding_.CloseAll(ZX_OK);
  hci_transport_binding_.RemoveAll();

  // Driver manager is given a raw pointer to this dynamically allocated object in Bind(), so when
  // DdkRelease() is called we need to free the allocated memory.
  delete this;
}

void Device::DdkSuspend(ddk::SuspendTxn txn) {
  zxlogf(DEBUG, "%s", __FUNCTION__);

  std::lock_guard lock(pending_request_lock_);
  unbound_ = true;

  txn.Reply(ZX_OK, 0);
}

void Device::ReadIsocInterfaces(usb_desc_iter_t* config_desc_iter) {
  usb_interface_descriptor_t* intf =
      usb_desc_iter_next_interface(config_desc_iter, /*skip_alt=*/false);
  while (intf) {
    if (intf->b_num_endpoints != kNumInterface1Endpoints) {
      zxlogf(ERROR, "USB interface %hhu alt setting %hhu does not have %hhu SCO endpoints",
             intf->i_interface, intf->b_alternate_setting, kNumInterface1Endpoints);
      break;
    }

    usb_endpoint_descriptor_t* in = nullptr;
    usb_endpoint_descriptor_t* out = nullptr;
    usb_endpoint_descriptor_t* endp = usb_desc_iter_next_endpoint(config_desc_iter);
    while (endp) {
      if (usb_ep_type(endp) == fuchsia_hardware_usb_descriptor::EndpointType::kIsochronous) {
        if (usb_ep_direction(endp) == USB_ENDPOINT_OUT) {
          out = endp;
        } else {
          ZX_ASSERT(usb_ep_direction(endp) == USB_ENDPOINT_IN);
          in = endp;
        }
      }
      endp = usb_desc_iter_next_endpoint(config_desc_iter);
    }
    if (!in || !out) {
      zxlogf(ERROR, "USB interface %hhu alt setting %hhu does not have in/out SCO endpoints",
             intf->i_interface, intf->b_alternate_setting);
      break;
    }
    isoc_endp_descriptors_.push_back(IsocEndpointDescriptors{.in = *in, .out = *out});

    intf = usb_desc_iter_next_interface(config_desc_iter, /*skip_alt=*/false);
  }
}

// Allocates a USB request and keeps track of how many requests have been allocated.
zx_status_t Device::InstrumentedRequestAlloc(usb_request_t** out, uint64_t data_size,
                                             uint8_t ep_address, size_t req_size) {
  atomic_fetch_add(&allocated_requests_count_, 1);
  return usb_request_alloc(out, data_size, ep_address, req_size);
}

// Releases a USB request and decrements the usage count.
// Signals a completion when all requests have been released.
void Device::InstrumentedRequestRelease(usb_request_t* req) {
  usb_request_release(req);
  size_t req_count = atomic_fetch_sub(&allocated_requests_count_, 1);
  zxlogf(TRACE, "remaining allocated requests: %zu", req_count - 1);
  // atomic_fetch_sub returns the value prior to being updated, so a value of 1 means that this is
  // the last request.
  if (req_count == 1) {
    sync_completion_signal(&requests_freed_completion_);
  }
}

// usb_request_callback is a hook that is inserted for every USB request
// which guarantees the following conditions:
// * No completions will be invoked during driver unbind.
// * pending_request_count shall indicate the number of requests outstanding.
// * pending_requests_completed shall be asserted when the number of requests pending equals zero.
// * Requests are properly freed during shutdown.
void Device::UsbRequestCallback(usb_request_t* req) {
  zxlogf(TRACE, "%s", __FUNCTION__);

  const uint8_t endp_addr = req->header.ep_address;

  auto epilogue = [this, endp_addr, function = __FUNCTION__](std::lock_guard<std::mutex>& guard) {
    size_t pending_request_count = std::atomic_fetch_sub(&pending_request_count_, 1);
    zxlogf(TRACE, "%s: pending requests: %zu", function, pending_request_count - 1);

    if (!isoc_endp_descriptors_.empty() &&
        endp_addr == isoc_endp_descriptors_[0].out.b_endpoint_address) {
      size_t prev_sco_count = std::atomic_fetch_sub(&pending_sco_write_request_count_, 1);
      if (prev_sco_count == 1) {
        pending_sco_write_request_count_0_cnd_.notify_one();
      }
    }
  };

  usb_callback_t callback;
  {
    std::lock_guard lock(pending_request_lock_);
    if (unbound_) {
      InstrumentedRequestRelease(req);
      epilogue(lock);
      return;
    }
    // Invoke the real completion if not shutting down.
    //
    // Request callback pointer is stored at the end of the usb_request_t after
    // other data that has been appended to the request by drivers elsewhere in the stack.
    // memcpy is necessary here to prevent undefined behavior since there are no guarantees
    // about the alignment of data that other drivers append to the usb_request_t.
    memcpy(static_cast<void*>(&callback),
           reinterpret_cast<unsigned char*>(req) + parent_req_size_ + sizeof(usb_req_internal_t),
           sizeof(callback));
  }
  // Our threading model allows a callback to immediately re-queue a request here
  // which would result in attempting to recursively lock pending_request_lock.
  // Unlocking the mutex is necessary to prevent a crash.
  callback(this, req);

  std::lock_guard lock(pending_request_lock_);
  epilogue(lock);
}

void Device::UsbRequestSend(usb_protocol_t* function, usb_request_t* req, usb_callback_t callback) {
  size_t parent_req_size;
  {
    std::lock_guard lock(pending_request_lock_);
    if (unbound_) {
      return;
    }
    std::atomic_fetch_add(&pending_request_count_, 1);

    if (!isoc_endp_descriptors_.empty() &&
        req->header.ep_address == isoc_endp_descriptors_[0].out.b_endpoint_address) {
      std::atomic_fetch_add(&pending_sco_write_request_count_, 1);
    }

    parent_req_size = parent_req_size_;
  };

  usb_request_complete_callback_t internal_completion = {
      .callback =
          [](void* ctx, usb_request_t* request) {
            static_cast<Device*>(ctx)->UsbRequestCallback(request);
          },
      .ctx = this};
  memcpy(reinterpret_cast<unsigned char*>(req) + parent_req_size + sizeof(usb_req_internal_t),
         static_cast<void*>(&callback), sizeof(callback));
  usb_request_queue(function, req, &internal_completion);
}

void Device::QueueAclReadRequestsLocked() {
  usb_request_t* req = nullptr;
  while ((req = usb_req_list_remove_head(&free_acl_read_reqs_, parent_req_size_)) != nullptr) {
    UsbRequestSend(&usb_, req, [](void* ctx, usb_request_t* req) {
      static_cast<Device*>(ctx)->HciAclReadComplete(req);
    });
  }
}

void Device::QueueScoReadRequestsLocked() {
  usb_request_t* req = nullptr;
  while ((req = usb_req_list_remove_head(&free_sco_read_reqs_, parent_req_size_)) != nullptr) {
    UsbRequestSend(&usb_, req, [](void* ctx, usb_request_t* req) {
      static_cast<Device*>(ctx)->HciScoReadComplete(req);
    });
  }
}

void Device::QueueInterruptRequestsLocked() {
  usb_request_t* req = nullptr;
  while ((req = usb_req_list_remove_head(&free_event_reqs_, parent_req_size_)) != nullptr) {
    UsbRequestSend(&usb_, req, [](void* ctx, usb_request_t* req) {
      static_cast<Device*>(ctx)->HciEventComplete(req);
    });
  }
}

void Device::SnoopChannelWriteLocked(enum SnoopPacketType type, bool is_received,
                                     const uint8_t* bytes, size_t length) {
  if (snoop_server_.has_value()) {
    if ((snoop_seq_ - acked_snoop_seq_) > kSeqNumMaxDiff) {
      if (!snoop_warning_emitted_) {
        zxlogf(
            WARNING,
            "Too many snoop packets not acked, current seq: %lu, acked seq: %lu, skipping snoop packets",
            snoop_seq_, acked_snoop_seq_);
        snoop_warning_emitted_ = true;
      }
      return;
    }
    // Reset log when the acked sequence number catches up.
    snoop_warning_emitted_ = false;

    fidl::Arena arena;
    auto builder = fhbt::wire::SnoopOnObservePacketRequest::Builder(arena);
    // auto fidl_vec = std::vector<uint8_t>(bytes, bytes + length);
    builder.direction(is_received ? fhbt::PacketDirection::kControllerToHost
                                  : fhbt::PacketDirection::kHostToController);

    switch (type) {
      case SnoopPacketType::kEvent: {
        ZX_DEBUG_ASSERT(is_received);
        builder.packet(fhbt::wire::SnoopPacket::WithEvent(
            arena, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(bytes), length)));
        break;
      }
      case SnoopPacketType::kCommand: {
        ZX_DEBUG_ASSERT(!is_received);
        builder.packet(fhbt::wire::SnoopPacket::WithCommand(
            arena, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(bytes), length)));
        break;
      }
      case SnoopPacketType::kAclData: {
        builder.packet(fhbt::wire::SnoopPacket::WithAcl(
            arena, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(bytes), length)));
        break;
      }
      case SnoopPacketType::kScoData: {
        builder.packet(fhbt::wire::SnoopPacket::WithSco(
            arena, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(bytes), length)));
        break;
      }
      case SnoopPacketType::kIsoData: {
        builder.packet(fhbt::wire::SnoopPacket::WithIso(
            arena, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(bytes), length)));
        break;
      }
      default:
        ZX_PANIC("Invalid snoop packet type");
    }

    builder.sequence(snoop_seq_++);
    fidl::OneWayStatus result =
        fidl::WireSendEvent(*snoop_server_)->OnObservePacket(builder.Build());
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send snoop packet: %s, unbinding snoop server",
             result.error().status_string());
      snoop_server_->Close(ZX_ERR_INTERNAL);
      snoop_server_.reset();
    }
  }
}

void Device::RemoveDeviceLocked() {
  if (!remove_requested_) {
    DdkAsyncRemove();
    remove_requested_ = true;
  }
}

void Device::HciEventComplete(usb_request_t* req) {
  zxlogf(TRACE, "bt-transport-usb: Event received");
  std::lock_guard lock(mutex_);

  if (req->response.status != ZX_OK) {
    HandleUsbResponseError(req, "hci event");
    return;
  }

  // Handle the interrupt as long as either HciTransport protocol or Snoop protocol is open.
  if (!snoop_server_.has_value() && hci_transport_binding_.size() == 0) {
    zxlogf(
        DEBUG,
        "bt-transport-usb: received hci event while HciTransport connection and Snoop client are "
        "closed");
    // Re-queue the HCI event USB request.
    zx_status_t status = usb_req_list_add_head(&free_event_reqs_, req, parent_req_size_);
    ZX_ASSERT(status == ZX_OK);
    QueueInterruptRequestsLocked();
    return;
  }

  void* buffer;
  zx_status_t status = usb_request_mmap(req, &buffer);
  if (status != ZX_OK) {
    zxlogf(ERROR, "bt-transport-usb: usb_req_mmap failed: %s", zx_status_get_string(status));
    return;
  }
  size_t length = req->response.actual;
  uint8_t* byte_buffer = static_cast<uint8_t*>(buffer);
  uint8_t event_parameter_total_size = byte_buffer[1];
  size_t packet_size = event_parameter_total_size + sizeof(HciEventHeader);

  // simple case - packet fits in received data
  if (event_buffer_offset_ == 0 && length >= sizeof(HciEventHeader)) {
    if (packet_size == length) {
      if (hci_transport_binding_.size()) {
        // When channel is not open, send event through HciTransport protocol events if the
        // protocol is open.
        auto fidl_vec_view = fidl::VectorView<uint8_t>::FromExternal(byte_buffer, length);
        fidl::Arena arena;

        hci_transport_binding_.ForEachBinding(
            [&](const fidl::ServerBinding<fhbt::HciTransport>& binding) {
              auto received_packet = fhbt::wire::ReceivedPacket::WithEvent(arena, fidl_vec_view);
              fidl::OneWayStatus result = fidl::WireSendEvent(binding)->OnReceive(received_packet);

              if (!result.ok()) {
                zxlogf(ERROR, "Failed to send event packet to host: %s", result.status_string());
              }
            });
      }

      SnoopChannelWriteLocked(SnoopPacketType::kEvent, /*is_received=*/true, byte_buffer, length);

      // Re-queue the HCI event USB request.
      status = usb_req_list_add_head(&free_event_reqs_, req, parent_req_size_);
      ZX_ASSERT(status == ZX_OK);
      QueueInterruptRequestsLocked();
      return;
    }
  }

  // complicated case - need to accumulate into hci->event_buffer

  if (event_buffer_offset_ + length > sizeof(event_buffer_)) {
    zxlogf(ERROR, "bt-transport-usb: event_buffer would overflow!");
    return;
  }

  memcpy(&event_buffer_[event_buffer_offset_], buffer, length);
  if (event_buffer_offset_ == 0) {
    event_buffer_packet_length_ = packet_size;
  } else {
    packet_size = event_buffer_packet_length_;
  }
  event_buffer_offset_ += length;

  // check to see if we have a full packet
  if (packet_size <= event_buffer_offset_) {
    zxlogf(TRACE,
           "bt-transport-usb: Accumulated full HCI event packet, sending on command & snoop "
           "channels.");
    if (hci_transport_binding_.size()) {
      // When channel is not open, send event through HciTransport protocol events if the protocol
      // is open.
      auto fidl_vec_view = fidl::VectorView<uint8_t>::FromExternal(event_buffer_, packet_size);
      fidl::Arena arena;

      hci_transport_binding_.ForEachBinding(
          [&](const fidl::ServerBinding<fhbt::HciTransport>& binding) {
            auto received_packet = fhbt::wire::ReceivedPacket::WithEvent(arena, fidl_vec_view);
            fidl::OneWayStatus result = fidl::WireSendEvent(binding)->OnReceive(received_packet);

            if (!result.ok()) {
              zxlogf(ERROR, "Failed to send event packet to host: %s", result.status_string());
            }
          });
    }

    SnoopChannelWriteLocked(SnoopPacketType::kEvent, /*is_received=*/true, event_buffer_,
                            packet_size);

    uint32_t remaining = static_cast<uint32_t>(event_buffer_offset_ - packet_size);
    memmove(event_buffer_, event_buffer_ + packet_size, remaining);
    event_buffer_offset_ = 0;
    event_buffer_packet_length_ = 0;
  } else {
    zxlogf(TRACE,
           "bt-transport-usb: Received incomplete chunk of HCI event packet. Appended to buffer.");
  }

  // Re-queue the HCI event USB request.
  status = usb_req_list_add_head(&free_event_reqs_, req, parent_req_size_);
  ZX_ASSERT(status == ZX_OK);
  QueueInterruptRequestsLocked();
}

void Device::HciAclReadComplete(usb_request_t* req) {
  zxlogf(TRACE, "bt-transport-usb: ACL frame received");
  std::lock_guard lock(mutex_);

  if (req->response.status == ZX_ERR_IO_INVALID) {
    zxlogf(TRACE, "bt-transport-usb: request stalled, ignoring.");
    zx_status_t status = usb_req_list_add_head(&free_acl_read_reqs_, req, parent_req_size_);
    ZX_DEBUG_ASSERT(status == ZX_OK);
    QueueAclReadRequestsLocked();

    return;
  }

  if (req->response.status != ZX_OK) {
    HandleUsbResponseError(req, "ACL read");
    return;
  }

  void* buffer;
  zx_status_t status = usb_request_mmap(req, &buffer);
  if (status != ZX_OK) {
    zxlogf(ERROR, "bt-transport-usb: usb_req_mmap failed: %s", zx_status_get_string(status));
    return;
  }

  if (hci_transport_binding_.size()) {
    // When channel is not open, send event through HciTransport protocol events if the protocol
    // is open.
    const uint8_t* byte_buffer = static_cast<uint8_t*>(buffer);
    auto fidl_vec = std::vector<uint8_t>(byte_buffer,
                                         byte_buffer + static_cast<uint32_t>(req->response.actual));

    hci_transport_binding_.ForEachBinding(
        [&](const fidl::ServerBinding<fhbt::HciTransport>& binding) {
          auto received_packet = fhbt::ReceivedPacket::WithAcl(fidl_vec);
          fit::result<fidl::OneWayError> result =
              fidl::SendEvent(binding)->OnReceive(received_packet);

          if (result.is_error()) {
            zxlogf(ERROR, "Failed to send ACL packet to host: %s",
                   result.error_value().status_string());
          }
        });
  } else {
    zxlogf(ERROR, "bt-transport-usb: ACL data received while HciTransport connection is closed");
  }

  // If the snoop client is available then try to write the packet even if HciTransport connection
  // was closed.
  SnoopChannelWriteLocked(SnoopPacketType::kAclData, /*is_received=*/true,
                          static_cast<uint8_t*>(buffer), req->response.actual);

  status = usb_req_list_add_head(&free_acl_read_reqs_, req, parent_req_size_);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  QueueAclReadRequestsLocked();
}

void Device::HciAclWriteComplete(usb_request_t* req) {
  std::lock_guard lock(mutex_);

  if (req->response.status != ZX_OK) {
    HandleUsbResponseError(req, "ACL write");
    return;
  }

  zx_status_t status = usb_req_list_add_tail(&free_acl_write_reqs_, req, parent_req_size_);
  ZX_DEBUG_ASSERT(status == ZX_OK);

  if (hci_transport_binding_.size()) {
    ZX_DEBUG_ASSERT(!acl_completer_queue_.empty());
    acl_completer_queue_.front().Reply();
    acl_completer_queue_.pop();
  }

  if (snoop_server_.has_value()) {
    void* buffer;
    zx_status_t status = usb_request_mmap(req, &buffer);
    if (status != ZX_OK) {
      zxlogf(ERROR, "bt-transport-usb: usb_req_mmap failed: %s", zx_status_get_string(status));
      return;
    }

    SnoopChannelWriteLocked(SnoopPacketType::kAclData, /*is_received=*/false,
                            static_cast<uint8_t*>(buffer), req->response.actual);
  }
}

void Device::HciScoReadComplete(usb_request_t* req) {
  zxlogf(TRACE, "SCO frame received");
  std::lock_guard lock(mutex_);

  // When the alt setting is changed, requests are cenceled and should not be requeued.
  if (req->response.status == ZX_ERR_CANCELED) {
    zx_status_t status = usb_req_list_add_head(&free_sco_read_reqs_, req, parent_req_size_);
    ZX_ASSERT(status == ZX_OK);
    return;
  }

  if (req->response.status == ZX_ERR_IO_INVALID) {
    zxlogf(TRACE, "SCO request stalled, ignoring.");
    zx_status_t status = usb_req_list_add_head(&free_sco_read_reqs_, req, parent_req_size_);
    ZX_DEBUG_ASSERT(status == ZX_OK);
    QueueScoReadRequestsLocked();
    return;
  }

  if (req->response.status != ZX_OK) {
    HandleUsbResponseError(req, "SCO read");
    return;
  }

  void* buffer;
  zx_status_t status = usb_request_mmap(req, &buffer);
  ZX_ASSERT_MSG(status == ZX_OK, "usb_req_mmap failed: %s", zx_status_get_string(status));

  sco_reassembler_.ProcessData({static_cast<uint8_t*>(buffer), req->response.actual});

  status = usb_req_list_add_head(&free_sco_read_reqs_, req, parent_req_size_);
  ZX_ASSERT(status == ZX_OK);
  QueueScoReadRequestsLocked();
}

void Device::OnScoReassemblerPacketLocked(cpp20::span<const uint8_t> packet) {
  if (sco_connection_server_.has_value()) {
    // When channel is not open, send event through HciTransport protocol events if the protocol
    // is open.
    auto fidl_vec =
        std::vector<uint8_t>(packet.data(), packet.data() + static_cast<uint32_t>(packet.size()));

    fit::result<fidl::OneWayError> result = sco_connection_server_->OnReceive(fidl_vec);

    if (result.is_error()) {
      zxlogf(ERROR, "Failed to send SCO packet to host: %s", result.error_value().status_string());
    }
  } else {
    zxlogf(ERROR, "SCO data received while no connection is available.");
  }

  // If the snoop client available then try to write the packet even if sco connection was closed.
  SnoopChannelWriteLocked(SnoopPacketType::kScoData, /*is_received=*/true, packet.data(),
                          packet.size());
}

void Device::HciScoWriteComplete(usb_request_t* req) {
  std::lock_guard lock(mutex_);

  if (req->response.status != ZX_OK) {
    HandleUsbResponseError(req, "SCO write");
    return;
  }

  zx_status_t status = usb_req_list_add_tail(&free_sco_write_reqs_, req, parent_req_size_);
  ZX_ASSERT(status == ZX_OK);

  // Reply the completer for this packet.
  if (sco_connection_server_.has_value()) {
    ZX_DEBUG_ASSERT(!sco_callback_queue_.empty());
    (sco_callback_queue_.front())();
    sco_callback_queue_.pop();
  }

  if (snoop_server_.has_value()) {
    void* buffer;
    zx_status_t status = usb_request_mmap(req, &buffer);
    if (status != ZX_OK) {
      zxlogf(ERROR, "usb_req_mmap failed: %s", zx_status_get_string(status));
      return;
    }

    SnoopChannelWriteLocked(SnoopPacketType::kScoData, /*is_received=*/false,
                            static_cast<uint8_t*>(buffer), req->response.actual);
  }
}

void Device::OnScoData(
    std::vector<uint8_t>& packet,
    fidl::Server<fuchsia_hardware_bluetooth::ScoConnection>::SendCompleter::Sync& completer) {
  std::lock_guard lock(mutex_);

  list_node_t* node = list_peek_head(&free_sco_write_reqs_);

  // We don't have enough reqs. Simply punt the channel read until later.
  if (!node) {
    zxlogf(ERROR, "Too many SCO packets in driver, closing SCO connection.");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  sco_callback_queue_.push([completer = completer.ToAsync()]() mutable { completer.Reply(); });

  const size_t& length = packet.size();

  if (length > kScoMaxFrameSize) {
    zxlogf(ERROR, "Acl packet too large (%zu), dropping", length);
    return;
  }

  node = list_remove_head(&free_sco_write_reqs_);
  // The mutex was held between the peek and the remove, so if we got this far the list must
  // have a node.
  ZX_ASSERT(node);

  usb_req_internal_t* req_int = containerof(node, usb_req_internal_t, node);
  usb_request_t* req = req_internal_to_usb_req(req_int, parent_req_size_);
  size_t result = usb_request_copy_to(req, packet.data(), length, 0);
  ZX_ASSERT(result == length);
  req->header.length = length;
  // The completion callback will not be called synchronously, so it is safe to hold mutex_ across
  // UsbRequestSend.
  UsbRequestSend(&usb_, req, [](void* ctx, usb_request_t* req) {
    static_cast<Device*>(ctx)->HciScoWriteComplete(req);
  });
}

void Device::OnScoStop() {
  uint8_t isoc_alt_setting = 0;
  {
    std::lock_guard lock(mutex_);
    isoc_alt_setting = isoc_alt_setting_;

    // Clear any partial SCO packets in the reassembler.
    sco_reassembler_.clear();
  }

  zx_status_t status =
      usb_cancel_all(&usb_, isoc_endp_descriptors_[isoc_alt_setting].in.b_endpoint_address);
  if (status != ZX_OK) {
    zxlogf(ERROR, "canceling isoc in requests failed with status: %s",
           zx_status_get_string(status));
  }

  status = usb_cancel_all(&usb_, isoc_endp_descriptors_[isoc_alt_setting].out.b_endpoint_address);
  if (status != ZX_OK) {
    zxlogf(ERROR, "canceling isoc out requests failed with status: %s",
           zx_status_get_string(status));
  }

  // New write requests are blocked at this point because ScoConnection is closed.
  // Pending write requests cannot be canceled because that would invalidate the host's free
  // controller buffer slot count (no HCI_Number_Of_Completed_Packets event would be received for
  // canceled packets). Instead, we must wait for them to complete normally.
  {
    std::unique_lock req_lock(pending_request_lock_);
    while (pending_sco_write_request_count_) {
      pending_sco_write_request_count_0_cnd_.wait(req_lock);
    }
  }

  status = usb_enable_endpoint(&usb_, &isoc_endp_descriptors_[isoc_alt_setting].in,
                               /*ss_com_desc=*/nullptr, /*enable=*/false);
  if (status != ZX_OK) {
    zxlogf(ERROR, "disabling isoc in endpoint failed with status: %s",
           zx_status_get_string(status));
  }

  status = usb_enable_endpoint(&usb_, &isoc_endp_descriptors_[isoc_alt_setting].out,
                               /*ss_com_desc=*/nullptr, /*enable=*/false);
  if (status != ZX_OK) {
    zxlogf(ERROR, "disabling isoc out endpoint failed with status: %s",
           zx_status_get_string(status));
  }
}

void Device::Send(SendRequest& request, SendCompleter::Sync& completer) {
  switch (request.Which()) {
    case fhbt::SentPacket::Tag::kIso:
      zxlogf(ERROR, "Unexpected ISO data packet.");
      break;

    case fhbt::SentPacket::Tag::kAcl: {
      list_node_t* node;
      const size_t& length = request.acl().value().size();

      node = list_peek_head(&free_acl_write_reqs_);

      // We don't have enough reqs. Simply punt the channel read until later.
      if (!node) {
        zxlogf(ERROR, "No usb request available in the buffer, closing HciTransport connection.");
        completer.Reply();
        hci_transport_binding_.RemoveBindings(this);
        break;
      }

      if (length > ACL_MAX_FRAME_SIZE) {
        zxlogf(ERROR, "Acl packet too large (%zu), dropping", length);
        break;
      }

      node = list_remove_head(&free_acl_write_reqs_);
      // At this point if we don't get a free node from |free_acl_write_reqs| that means that
      // they were cleaned up in hci_release(). Just drop the packet.
      if (!node) {
        completer.Reply();
        return;
      }

      usb_req_internal_t* req_int = containerof(node, usb_req_internal_t, node);
      usb_request_t* req = req_internal_to_usb_req(req_int, parent_req_size_);
      size_t result = usb_request_copy_to(req, request.acl().value().data(), length, 0);
      ZX_ASSERT(result == length);
      req->header.length = length;

      acl_completer_queue_.push(completer.ToAsync());

      UsbRequestSend(&usb_, req, [](void* ctx, usb_request_t* req) mutable {
        static_cast<Device*>(ctx)->HciAclWriteComplete(req);
      });

      break;
    }

    case fhbt::SentPacket::Tag::kCommand: {
      // There's no callback for command write operation.
      completer.Reply();
      {
        std::lock_guard lock(mutex_);
        SnoopChannelWriteLocked(SnoopPacketType::kCommand, /*is_received=*/false,
                                request.command().value().data(), request.command().value().size());
      }

      if (request.command().value().size() > fhbt::kCommandMax) {
        zxlogf(WARNING, "Command packet too large (%zu), dropping",
               request.command().value().size());
        break;
      }

      zx_status_t control_status = usb_control_out(
          &usb_, USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_DEVICE, 0, 0, 0, ZX_TIME_INFINITE,
          request.command().value().data(), request.command().value().size());
      if (control_status != ZX_OK) {
        zxlogf(ERROR, "usb_control_out failed: %s", zx_status_get_string(control_status));
        break;
      }
      break;
    }

    default:
      zxlogf(ERROR, "Unknown packet type: %zu", request.Which());
  }
}

void Device::AckReceive(AckReceiveCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/349616746): Implement flow control based on AckReceive.
}

void Device::ConfigureSco(ConfigureScoRequest& request, ConfigureScoCompleter::Sync& completer) {
  if (request.connection().has_value()) {
    if (sco_connection_server_.has_value()) {
      zxlogf(WARNING,
             "A ScoConnection server end received, but connection has already established.");
    } else {
      sco_connection_server_.emplace(
          dispatcher_, std::move(request.connection().value()),
          [this](std::vector<uint8_t>& packet,
                 fidl::Server<fuchsia_hardware_bluetooth::ScoConnection>::SendCompleter::Sync&
                     completer) { OnScoData(packet, completer); },
          [this] { OnScoStop(); }, [this](fidl::UnbindInfo) { sco_connection_server_.reset(); });
    }
  }

  if (!request.coding_format().has_value() || !request.sample_rate().has_value() ||
      !request.encoding().has_value()) {
    zxlogf(ERROR, "Required field missing: %s %s %s",
           request.coding_format().has_value() ? "" : "coding_format",
           request.sample_rate().has_value() ? "" : "sample_rate",
           request.encoding().has_value() ? "" : "encoding");
    return;
  }

  // NOLINTBEGIN: bugprone-unchecked-optional-access (false positive)
  const auto& coding_format = request.coding_format().value();
  const auto& sample_rate = request.sample_rate().value();
  const auto& encoding = request.encoding().value();
  // NOLINTEND: bugprone-unchecked-optional-access

  uint8_t new_alt_setting = 0;

  // Only the settings for 1 voice channel are supported.
  // MSBC uses alt setting 1 because few controllers support alt setting 6.
  // See Core Spec v5.3, Vol 4, Part B, Sec 2.1.1 for settings table.
  if (coding_format == fhbt::ScoCodingFormat::kMsbc ||
      (sample_rate == fhbt::ScoSampleRate::kKhz8 && encoding == fhbt::ScoEncoding::kBits8)) {
    new_alt_setting = 1;
  } else if (sample_rate == fhbt::ScoSampleRate::kKhz8 && encoding == fhbt::ScoEncoding::kBits16) {
    new_alt_setting = 2;
  } else if (sample_rate == fhbt::ScoSampleRate::kKhz16 && encoding == fhbt::ScoEncoding::kBits16) {
    new_alt_setting = 4;
  } else {
    zxlogf(WARNING, "SCO configuration not supported");
    return;
  }

  std::lock_guard lock(mutex_);

  if (new_alt_setting >= isoc_endp_descriptors_.size()) {
    zxlogf(ERROR, "isoc alt setting %hhu not supported, cannot configure SCO", new_alt_setting);
    return;
  }

  zx_status_t status = usb_set_interface(&usb_, kIsocInterfaceNum, new_alt_setting);
  if (status != ZX_OK) {
    zxlogf(ERROR, "usb_set_interface failed with status: %s", zx_status_get_string(status));
    return;
  }

  // Enable new endpoints.
  status = usb_enable_endpoint(&usb_, &isoc_endp_descriptors_[new_alt_setting].in,
                               /*ss_com_desc=*/nullptr, /*enable=*/true);
  if (status != ZX_OK) {
    zxlogf(ERROR, "usb_enable_endpoint failed with status: %s", zx_status_get_string(status));
    return;
  }
  status = usb_enable_endpoint(&usb_, &isoc_endp_descriptors_[new_alt_setting].out,
                               /*ss_com_desc=*/nullptr, /*enable=*/true);
  if (status != ZX_OK) {
    zxlogf(ERROR, "usb_enable_endpoint failed with status: %s", zx_status_get_string(status));
    return;
  }

  isoc_alt_setting_ = new_alt_setting;

  // Clear any partial SCO packets in the reassembler.
  sco_reassembler_.clear();
  QueueScoReadRequestsLocked();
}

void Device::handle_unknown_method(
    ::fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata,
    ::fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "Unknown method in HciTransport protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void Device::AcknowledgePackets(AcknowledgePacketsRequest& request,
                                AcknowledgePacketsCompleter::Sync& completer) {
  acked_snoop_seq_ = std::max(request.sequence(), acked_snoop_seq_);
}

void Device::handle_unknown_method(

    ::fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::Snoop> metadata,
    ::fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "Unknown method in fidl::Server<fuchsia_hardware_bluetooth::Snoop>");
}

zx_status_t Device::AllocBtUsbPackets(int limit, uint64_t data_size, uint8_t ep_address,
                                      size_t req_size, list_node_t* list) {
  zx_status_t status;
  for (int i = 0; i < limit; i++) {
    usb_request_t* req;
    status = InstrumentedRequestAlloc(&req, data_size, ep_address, req_size);
    if (status != ZX_OK) {
      return status;
    }
    status = usb_req_list_add_head(list, req, parent_req_size_);
    if (status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

void Device::OnBindFailure(zx_status_t status, const char* msg) {
  zxlogf(ERROR, "bind failed due to %s: %s", msg, zx_status_get_string(status));
  DdkRelease();
}

void Device::HandleUsbResponseError(usb_request_t* req, const char* req_description) {
  zx_status_t status = req->response.status;
  InstrumentedRequestRelease(req);
  zxlogf(ERROR, "%s request completed with error status (%s). Removing device", req_description,
         zx_status_get_string(status));
  RemoveDeviceLocked();
}

namespace {

// A lambda is used to create an empty instance of zx_driver_ops_t.
zx_driver_ops_t usb_bt_hci_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Device::Create;
  return ops;
}();

}  // namespace

}  // namespace bt_transport_usb

ZIRCON_DRIVER(bt_transport_usb, bt_transport_usb::usb_bt_hci_driver_ops, "zircon", "0.1");
