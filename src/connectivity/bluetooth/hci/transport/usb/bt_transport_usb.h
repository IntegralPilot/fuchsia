// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_HCI_TRANSPORT_USB_BT_TRANSPORT_USB_H_
#define SRC_CONNECTIVITY_BLUETOOTH_HCI_TRANSPORT_USB_BT_TRANSPORT_USB_H_

#include <fidl/fuchsia.hardware.bluetooth/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/sync/completion.h>

#include <queue>

#include <ddktl/device.h>
#include <usb/usb.h>

#include "packet_reassembler.h"

namespace bt_transport_usb {

class Device;

// This class exists (rather than the implementation being on Device) to avoid the conflict between
// the two AckReceive methods which have the exact same signature.
class ScoConnectionServer : public fidl::Server<fuchsia_hardware_bluetooth::ScoConnection> {
  using SendHandler = fit::function<void(std::vector<uint8_t>&, SendCompleter::Sync&)>;
  using StopHandler = fit::function<void(void)>;

 public:
  ScoConnectionServer(async_dispatcher_t* dispatcher,
                      fidl::ServerEnd<fuchsia_hardware_bluetooth::ScoConnection> server_end,
                      SendHandler send_handler, StopHandler stop_handler,
                      fit::function<void(fidl::UnbindInfo)> close_handler);

  fit::result<fidl::OneWayError> OnReceive(const fuchsia_hardware_bluetooth::ScoPacket& payload) {
    return fidl::SendEvent(binding_)->OnReceive(payload);
  }

  // fuchsia_hardware_bluetooth::ScoConnection overrides.
  void Send(SendRequest& request, SendCompleter::Sync& completer) override;
  void AckReceive(AckReceiveCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void handle_unknown_method(
      ::fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::ScoConnection> metadata,
      ::fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  SendHandler send_handler_;
  StopHandler stop_handler_;
  fidl::ServerBinding<fuchsia_hardware_bluetooth::ScoConnection> binding_;
};

// See ddk::Device in ddktl/device.h
using DeviceType = ddk::Device<Device, ddk::GetProtocolable, ddk::Suspendable, ddk::Unbindable>;

// This driver can be bound to devices requiring the ZX_PROTOCOL_BT_TRANSPORT protocol, but this
// driver actually implements the ZX_PROTOCOL_BT_HCI protocol. Drivers that bind to
// ZX_PROTOCOL_BT_HCI should never bind to this driver directly, but instead bind to a vendor
// driver.
//
// BtHciProtocol is not a ddk::base_protocol because vendor drivers proxy requests to this driver.
class Device final : public DeviceType,
                     public fidl::Server<fuchsia_hardware_bluetooth::HciTransport>,
                     public fidl::Server<fuchsia_hardware_bluetooth::Snoop> {
 public:
  // If |dispatcher| is non-null, it will be used instead of a new work thread.
  // tests.
  explicit Device(zx_device_t* parent, async_dispatcher_t* dispatcher);

  // Static bind function for the ZIRCON_DRIVER() declaration. Binds a Device and passes ownership
  // to the driver manager.
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  // Constructor for tests to inject a dispatcher for the work thread.
  static zx_status_t Create(zx_device_t* parent, async_dispatcher_t* dispatcher);

  // Adds the device.
  zx_status_t Bind();

  // This is only for test purpose, mock_ddk doesn't provide a way for the test to connect to a fidl
  // protocol exposed from the driver's outgoing directory.
  // TODO(https://fxbug.dev/332333517): Remove this function after DFv2 migration.
  void ConnectHciTransport(fidl::ServerEnd<fuchsia_hardware_bluetooth::HciTransport> server_end);

  // This is only for test purpose, mock_ddk doesn't provide a way for the test to connect to a fidl
  // protocol exposed from the driver's outgoing directory.
  // TODO(https://fxbug.dev/332333517): Remove this function after DFv2 migration.
  void ConnectSnoop(fidl::ServerEnd<fuchsia_hardware_bluetooth::Snoop> server_end);

  // Methods required by DDK mixins:
  zx_status_t DdkGetProtocol(uint32_t proto_id, void* out);
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease();
  void DdkSuspend(ddk::SuspendTxn txn);

  // fuchsia_hardware_bluetooth::HciTransport protocol overrides.
  void Send(SendRequest& request, SendCompleter::Sync& completer) override;
  void AckReceive(AckReceiveCompleter::Sync& completer) override;
  void ConfigureSco(ConfigureScoRequest& request, ConfigureScoCompleter::Sync& completer) override;
  void handle_unknown_method(
      ::fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata,
      ::fidl::UnknownMethodCompleter::Sync& completer) override;

  // fuchsia_hardware_bluetooth::Snoop protocol overrides.
  void AcknowledgePackets(AcknowledgePacketsRequest& request,
                          AcknowledgePacketsCompleter::Sync& completer) override;
  void handle_unknown_method(
      ::fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::Snoop> metadata,
      ::fidl::UnknownMethodCompleter::Sync& completer) override;

  std::mutex& mutex() { return mutex_; }

 private:
  struct IsocEndpointDescriptors {
    usb_endpoint_descriptor_t in;
    usb_endpoint_descriptor_t out;
  };

  enum class SnoopPacketType {
    kCommand,
    kEvent,
    kAclData,
    kScoData,
    kIsoData,
  };

  using usb_callback_t = void (*)(void*, usb_request_t*);

  void ReadIsocInterfaces(usb_desc_iter_t* config_desc_iter);

  zx_status_t InstrumentedRequestAlloc(usb_request_t** out, uint64_t data_size, uint8_t ep_address,
                                       size_t req_size);

  void InstrumentedRequestRelease(usb_request_t* req);

  void UsbRequestCallback(usb_request_t* req);

  void UsbRequestSend(usb_protocol_t* function, usb_request_t* req, usb_callback_t callback);

  void QueueAclReadRequestsLocked() __TA_REQUIRES(mutex_);

  void QueueScoReadRequestsLocked() __TA_REQUIRES(mutex_);

  void QueueInterruptRequestsLocked() __TA_REQUIRES(mutex_);

  void SnoopChannelWriteLocked(enum SnoopPacketType type, bool is_received, const uint8_t* bytes,
                               size_t length) __TA_REQUIRES(mutex_);

  // Requests removal of this device. Idempotent.
  void RemoveDeviceLocked() __TA_REQUIRES(mutex_);

  void HciEventComplete(usb_request_t* req);

  void HciAclReadComplete(usb_request_t* req);

  void HciAclWriteComplete(usb_request_t* req);

  void HciScoReadComplete(usb_request_t* req);

  // Called by sco_reassembler_ when a packet is recombined.
  // This method assumes mutex_ is held during invocation. We disable thread safety analysis because
  // the PacketReassembler callback is too complex for Clang. sco_reassembler_ requires mutex_, so
  // this method effectively requires mutex_.
  void OnScoReassemblerPacketLocked(cpp20::span<const uint8_t> packet)
      __TA_NO_THREAD_SAFETY_ANALYSIS;

  void HciScoWriteComplete(usb_request_t* req);

  void OnScoData(std::vector<uint8_t>&,
                 fidl::Server<fuchsia_hardware_bluetooth::ScoConnection>::SendCompleter::Sync&);
  void OnScoStop();

  zx_status_t AllocBtUsbPackets(int limit, uint64_t data_size, uint8_t ep_address, size_t req_size,
                                list_node_t* list);

  // Called upon Bind failure.
  void OnBindFailure(zx_status_t status, const char* msg);

  void HandleUsbResponseError(usb_request_t* req, const char* req_description)
      __TA_REQUIRES(mutex_);

  std::mutex mutex_;

  usb_protocol_t usb_ __TA_GUARDED(mutex_);

  std::optional<async::Loop> loop_;
  // In production, this is loop_.dispatcher(). In tests, this is the test dispatcher.
  async_dispatcher_t* dispatcher_ = nullptr;

  // Set during binding and never modified after.
  std::optional<usb_endpoint_descriptor_t> bulk_out_endp_desc_;
  std::optional<usb_endpoint_descriptor_t> bulk_in_endp_desc_;
  std::optional<usb_endpoint_descriptor_t> intr_endp_desc_;

  // The alternate setting of the ISOC (SCO) interface.
  uint8_t isoc_alt_setting_ __TA_GUARDED(mutex_) = 0;

  // Set during bind, never modified afterwards.
  std::vector<IsocEndpointDescriptors> isoc_endp_descriptors_;

  // for accumulating HCI events
  uint8_t event_buffer_[fuchsia_hardware_bluetooth::kEventMax] __TA_GUARDED(mutex_);
  size_t event_buffer_offset_ __TA_GUARDED(mutex_) = 0u;
  size_t event_buffer_packet_length_ __TA_GUARDED(mutex_) = 0u;

  PacketReassembler<fuchsia_hardware_bluetooth::kScoPacketMax> sco_reassembler_
      __TA_GUARDED(mutex_);

  // pool of free USB requests
  list_node_t free_event_reqs_ __TA_GUARDED(mutex_);
  list_node_t free_acl_read_reqs_ __TA_GUARDED(mutex_);
  list_node_t free_acl_write_reqs_ __TA_GUARDED(mutex_);
  list_node_t free_sco_read_reqs_ __TA_GUARDED(mutex_);
  list_node_t free_sco_write_reqs_ __TA_GUARDED(mutex_);

  // Enqueue the completer when queuing a ACL packet send request to with |UsbRequestSend|. Take out
  // a completer and reply when the packet is sent(i.e. |HciAclWriteComplete| is called).
  std::queue<SendCompleter::Async> acl_completer_queue_;

  // When SCO data packet gets into sco_send_queue_, the corresponding completer will be queued
  // here, and the callback will be dequeued and called when that packet is sent.
  std::queue<fit::function<void(void)>> sco_callback_queue_;

  size_t parent_req_size_ = 0u;
  std::atomic_size_t allocated_requests_count_ = 0u;
  std::atomic_size_t pending_request_count_ = 0u;
  std::atomic_size_t pending_sco_write_request_count_ = 0u;
  std::condition_variable pending_sco_write_request_count_0_cnd_;
  sync_completion_t requests_freed_completion_;

  // Whether or not we are being unbound.
  bool unbound_ __TA_GUARDED(pending_request_lock_) = false;

  // Set to true when RemoveDeviceLocked() has been called.
  bool remove_requested_ __TA_GUARDED(mutex_) = false;

  // Locked while sending a request, when handling a request callback, or when unbinding.
  // Useful when any operation needs to terminate if the driver is being unbound.
  // Also used to receive condition signals from request callbacks (e.g. indicating 0 pending
  // requests remain).
  // This is separate from mutex_ so that request operations don't need to acquire mutex_ (which
  // may degrade performance).
  std::mutex pending_request_lock_ __TA_ACQUIRED_AFTER(mutex_);

  std::optional<component::OutgoingDirectory> outgoing_;

  // When |snoop_seq_| - |acked_snoop_seq_| > |kSeqNumMaxDiff|, it means that the receiver of
  // snoop packets can't catch up the speed that the driver sends packets. Drop snoop packets if
  // this happens.
  static constexpr uint64_t kSeqNumMaxDiff = 20;

  // This is the sequence number of the snoop packets that this driver sends out. The receiver will
  // need to ack the packets with their sequence number, and the driver will record the highest
  // acked sequence number with |acked_snoop_seq_|.
  uint64_t snoop_seq_ = 0;
  uint64_t acked_snoop_seq_ = 0;

  // When |acked_snoop_seq_| fails to catch up |snoop_seq_|, the driver will drop the snoop packets
  // until |acked_snoop_seq_| catches up. Only one warning log should be emitted during this
  // process, instead of for all the snoop packets after the failure starts happening. This boolean
  // is to mark whether the log has been emitted in each failure.
  bool snoop_warning_emitted_ = false;

  std::optional<ScoConnectionServer> sco_connection_server_;
  fidl::ServerBindingGroup<fuchsia_hardware_bluetooth::HciTransport> hci_transport_binding_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_bluetooth::Snoop>> snoop_server_;
};

}  // namespace bt_transport_usb

#endif  // SRC_CONNECTIVITY_BLUETOOTH_HCI_TRANSPORT_USB_BT_TRANSPORT_USB_H_
