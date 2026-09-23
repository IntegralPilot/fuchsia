// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_HCI_PASSTHROUGH_PASSTHROUGH_H_
#define SRC_CONNECTIVITY_BLUETOOTH_HCI_PASSTHROUGH_PASSTHROUGH_H_

#include <assert.h>
#include <fidl/fuchsia.hardware.bluetooth/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/status.h>

namespace bt::passthrough {

class PassthroughDevice
    : public fdf::DriverBase2,
      public fidl::WireServer<fuchsia_hardware_bluetooth::Vendor>,
      public fidl::WireServer<fuchsia_hardware_bluetooth::HciTransport>,
      public fidl::WireAsyncEventHandler<fuchsia_hardware_bluetooth::HciTransport> {
 public:
  PassthroughDevice() : DriverBase2("bt_hci_passthrough") {}

  ~PassthroughDevice() override;

 private:
  // DriverBase2 overrides:
  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;

  // WireServer<Vendor> overrides:
  void GetFeatures(GetFeaturesCompleter::Sync& completer) override;
  void EncodeCommand(EncodeCommandRequestView request,
                     EncodeCommandCompleter::Sync& completer) override;
  void OpenHci(OpenHciCompleter::Sync& completer) override;
  void OpenHciTransport(OpenHciTransportCompleter::Sync& completer) override;
  void OpenSnoop(OpenSnoopCompleter::Sync& completer) override;
  void GetCrashParameters(GetCrashParametersCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::Vendor> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // WireServer<HciTransport> overrides:
  void Send(::fuchsia_hardware_bluetooth::wire::SentPacket* request,
            SendCompleter::Sync& completer) override;
  void AckReceive(AckReceiveCompleter::Sync& completer) override;
  void ConfigureSco(::fuchsia_hardware_bluetooth::wire::HciTransportConfigureScoRequest* request,
                    ConfigureScoCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // WireAsyncEventHandler<HciTransport> overrides:
  void OnReceive(
      ::fidl::WireEvent<::fuchsia_hardware_bluetooth::HciTransport::OnReceive>* event) override;
  void on_fidl_error(::fidl::UnbindInfo error) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<::fuchsia_hardware_bluetooth::HciTransport> metadata) override;

  zx_status_t ConnectToHciTransportFidlProtocol();

  fidl::WireClient<fuchsia_hardware_bluetooth::HciTransport> hci_transport_client_;
  fidl::ServerBindingGroup<fuchsia_hardware_bluetooth::Vendor> vendor_binding_group_;
  fidl::ServerBindingGroup<fuchsia_hardware_bluetooth::HciTransport> hci_transport_server_bindings_;
  std::shared_ptr<fdf::Namespace> incoming_;
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
};

}  // namespace bt::passthrough

#endif  //  SRC_CONNECTIVITY_BLUETOOTH_HCI_PASSTHROUGH_PASSTHROUGH_H_
