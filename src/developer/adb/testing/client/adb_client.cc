// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/adb/testing/client/adb_client.h"

#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <filesystem>
#include <iostream>
#include <string>
#include <vector>

#include <usb/usb.h>

#include "src/developer/adb/third_party/adb/adb-protocol.h"
#include "src/developer/adb/third_party/adb/types.h"

namespace fdescriptor = fuchsia_hardware_usb_descriptor;

AdbClientImpl::AdbClientImpl(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

void AdbClientImpl::Setup(AdbClientImpl::SetupCompleter::Sync& completer) {
  FX_LOGS(INFO) << "AdbClientImpl::Setup called.";
  if (usb_connected_) {
    completer.Reply(fit::ok());
    return;
  }

  if (auto status = DiscoverAndConnect(); status != ZX_OK) {
    FX_LOGS(ERROR) << "DiscoverAndConnect failed: " << zx_status_get_string(status);
    completer.Reply(fit::error(status));
    return;
  }
  completer.Reply(fit::ok());
}

void AdbClientImpl::Connect(AdbClientImpl::ConnectCompleter::Sync& completer) {
  FX_LOGS(INFO) << "AdbClientImpl::Connect called.";
  if (handshake_complete_) {
    completer.Reply(fit::ok());
    return;
  }

  if (!usb_connected_) {
    FX_LOGS(ERROR) << "USB not connected. Call Setup() first.";
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }

  // Construct A_CNXN packet
  apacket* p = get_apacket();
  FX_LOGS(INFO) << "Sending A_CNXN packet";
  p->msg.command = A_CNXN;
  p->msg.arg0 = A_VERSION;
  p->msg.arg1 = MAX_PAYLOAD;
  const char* payload = "host::\0";
  size_t len = strlen(payload) + 1;
  p->msg.data_length = static_cast<uint32_t>(len);
  p->payload.resize(len);
  memcpy(p->payload.data(), payload, len);
  p->msg.data_check = calculate_apacket_checksum(p);
  p->msg.magic = p->msg.command ^ 0xffffffff;

  if (auto status = SendPacket(p); status != ZX_OK) {
    FX_LOGS(ERROR) << "SendPacket failed: " << zx_status_get_string(status);
    put_apacket(p);
    completer.Reply(fit::error(status));
    return;
  }
  put_apacket(p);

  // Save the completer and wait for the response.
  connect_completer_ = completer.ToAsync();

  // Queue a read request for the handshake response.
  if (auto status = QueueReadRequest(); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to queue read request: " << zx_status_get_string(status);
    connect_completer_->Reply(fit::error(status));
    connect_completer_.reset();
  }
}

void AdbClientImpl::ExecuteCommand(AdbClientImpl::ExecuteCommandRequest& request,
                                   AdbClientImpl::ExecuteCommandCompleter::Sync& completer) {
  FX_LOGS(INFO) << "AdbClientImpl::ExecuteCommand called with " << request.command();
  // TODO(puneetha): Support multiple simultaneous commands.
  if (execute_completer_) {
    completer.Reply(fit::error(ZX_ERR_SHOULD_WAIT));
    return;
  }
  execute_completer_ = completer.ToAsync();
  command_output_.clear();

  // Open a shell stream for the command.
  // Use a new local ID for each command session.
  local_id_++;
  std::string cmd = "shell:" + std::string(request.command());

  apacket* p = get_apacket();
  p->msg.command = A_OPEN;
  p->msg.arg0 = local_id_;
  p->msg.arg1 = 0;
  p->msg.data_length = static_cast<uint32_t>(cmd.length() + 1);
  p->payload.resize(p->msg.data_length);
  memcpy(p->payload.data(), cmd.c_str(), cmd.length());
  p->payload[cmd.length()] = '\0';
  p->msg.data_check = calculate_apacket_checksum(p);
  p->msg.magic = p->msg.command ^ 0xffffffff;

  // Ensure we are listening for the response BEFORE we send the request.
  if (auto status = QueueReadRequest(); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to queue read request: " << zx_status_get_string(status);
  }

  if (auto status = SendPacket(p); status != ZX_OK) {
    execute_completer_->Reply(fit::error(status));
    execute_completer_.reset();
    put_apacket(p);
    return;
  }
  put_apacket(p);
}

void AdbClientImpl::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_testing_adb::Client> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Unknown method called: " << metadata.method_ordinal;
}

void AdbClientImpl::OnCompletion(
    fidl::Event<fuchsia_hardware_usb_endpoint::Endpoint::OnCompletion>& event) {
  for (auto& completion : event.completion()) {
    zx_status_t status = completion.status().value_or(ZX_OK);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Bulk completion error: " << zx_status_get_string(status);
      if (connect_completer_) {
        connect_completer_->Reply(fit::error(status));
        connect_completer_.reset();
      }
      continue;
    }

    if (!completion.request().has_value() || !completion.request()->data().has_value() ||
        completion.request()->data()->empty()) {
      continue;
    }

    auto& region = (*completion.request()->data())[0];
    if (!region.buffer().has_value() || !region.buffer()->data().has_value()) {
      continue;
    }

    const uint8_t* data = region.buffer()->data()->data();
    size_t len = static_cast<size_t>(completion.transfer_size().value_or(0));

    if (expecting_payload_bytes_ > 0) {
      size_t to_append = std::min(len, expecting_payload_bytes_);
      command_output_.append(reinterpret_cast<const char*>(data), to_append);
      expecting_payload_bytes_ -= to_append;

      if (expecting_payload_bytes_ == 0) {
        apacket* ack = get_apacket();
        ack->msg.command = A_OKAY;
        ack->msg.arg0 = local_id_;
        ack->msg.arg1 = remote_id_;
        ack->msg.magic = ack->msg.command ^ 0xffffffff;
        SendPacket(ack);
        put_apacket(ack);
      }

      if (auto status = QueueReadRequest(); status != ZX_OK) {
        FX_LOGS(ERROR) << "Failed to re-queue read request: " << zx_status_get_string(status);
      }
      continue;
    }

    if (len < sizeof(amessage)) {
      FX_LOGS(ERROR) << "Received packet too small for ADB header: " << len;
      continue;
    }

    amessage msg;
    memcpy(&msg, data, sizeof(amessage));

    FX_LOGS(INFO) << "Received ADB packet: cmd=0x" << std::hex << msg.command << " arg0=0x"
                  << msg.arg0 << " arg1=0x" << msg.arg1 << " len=" << std::dec << msg.data_length;

    if (msg.command == A_CNXN) {
      FX_LOGS(INFO) << "Received A_CNXN response";
      handshake_complete_ = true;
      if (connect_completer_) {
        connect_completer_->Reply(fit::ok());
        connect_completer_.reset();
      }
    } else if (msg.command == A_AUTH) {
      FX_LOGS(INFO) << "Received A_AUTH response - auth not supported yet";
      if (connect_completer_) {
        connect_completer_->Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
        connect_completer_.reset();
      }
    } else if (msg.command == A_OKAY) {
      if (msg.arg1 == local_id_) {
        if (remote_id_ == 0) {
          remote_id_ = msg.arg0;
          FX_LOGS(INFO) << "ADB Stream opened (local=" << local_id_ << ", remote=" << remote_id_
                        << ")";
        } else {
          FX_LOGS(INFO) << "ADB Device acknowledged write (local=" << local_id_ << ")";
        }
      }
    } else if (msg.command == A_WRTE) {
      if (msg.arg1 == local_id_) {
        size_t payload_len = msg.data_length;
        if (payload_len > 0) {
          size_t payload_in_packet = len - sizeof(amessage);
          if (payload_in_packet > 0) {
            size_t to_append = std::min(payload_in_packet, payload_len);
            command_output_.append(reinterpret_cast<const char*>(data + sizeof(amessage)),
                                   to_append);
            expecting_payload_bytes_ = payload_len - to_append;
          } else {
            expecting_payload_bytes_ = payload_len;
          }
        }

        if (expecting_payload_bytes_ == 0) {
          apacket* ack = get_apacket();
          ack->msg.command = A_OKAY;
          ack->msg.arg0 = local_id_;
          ack->msg.arg1 = remote_id_;
          ack->msg.magic = ack->msg.command ^ 0xffffffff;
          SendPacket(ack);
          put_apacket(ack);
        }
      }
    } else if (msg.command == A_CLSE) {
      if (msg.arg1 == local_id_) {
        FX_LOGS(INFO) << "ADB Stream closed. Output size: " << command_output_.length();
        if (execute_completer_) {
          execute_completer_->Reply(fit::ok(std::move(command_output_)));
          execute_completer_.reset();
        }
      }
    } else {
      FX_LOGS(INFO) << "Received unexpected command: " << std::hex << msg.command;
    }

    // Always queue a new read request to keep the bulk pipe filled.
    if (auto status = QueueReadRequest(); status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to re-queue read request: " << zx_status_get_string(status);
    }
  }
}

void AdbClientImpl::on_fidl_error(fidl::UnbindInfo error) {
  FX_LOGS(ERROR) << "Bulk endpoint FIDL error: " << error;
  if (connect_completer_) {
    connect_completer_->Reply(fit::error(error.status()));
    connect_completer_.reset();
  }
}

zx_status_t AdbClientImpl::DiscoverAndConnect() {
  FX_LOGS(INFO) << "Discovering ADB devices...";

  // Log all available services for debugging
  for (const char* svc_name :
       {fuchsia_hardware_usb_device::Service::Name, fuchsia_hardware_usb::UsbService::Name}) {
    std::string path = std::string("/svc/") + svc_name;
    if (std::filesystem::exists(path)) {
      FX_LOGS(INFO) << "Instances in " << path << ":";
      for (const auto& entry : std::filesystem::directory_iterator(path)) {
        FX_LOGS(INFO) << "  " << entry.path().filename().string();
      }
    } else {
      FX_LOGS(INFO) << path << " does not exist";
    }
  }

  // Discover devices via ServiceFS
  auto base_svc_dir = component::OpenDirectory("/svc");
  if (base_svc_dir.is_error()) {
    FX_LOGS(ERROR) << "Failed to open /svc: " << base_svc_dir.status_string();
    return base_svc_dir.status_value();
  }

  std::string service_path = fuchsia_hardware_usb_device::Service::Name;
  FX_LOGS(INFO) << "Watching for devices in /svc/" << service_path;

  auto watcher_dir = component::OpenDirectoryAt(base_svc_dir->borrow(), service_path);
  if (watcher_dir.is_ok()) {
    auto result = device_watcher::WatchDirectoryForItems<zx_status_t>(
        watcher_dir->borrow(),
        [this, &base_svc_dir](std::string_view instance) -> std::optional<zx_status_t> {
          if (instance == "." || instance == "..") {
            return std::nullopt;
          }

          FX_LOGS(INFO) << "Trying service instance: " << instance;
          auto open_result = component::OpenServiceAt<fuchsia_hardware_usb_device::Service>(
              base_svc_dir->borrow(), instance);
          if (open_result.is_error()) {
            return std::nullopt;
          }

          auto connect_result = open_result->connect_device();
          if (connect_result.is_error()) {
            return std::nullopt;
          }

          fidl::SyncClient device{std::move(connect_result.value())};
          if (auto status = ProcessDevice(device, instance); status == ZX_OK) {
            FX_LOGS(INFO) << "Found ADB device via service: " << instance;
            return ZX_OK;
          }
          return std::nullopt;
        });

    if (result.is_ok()) {
      return ZX_OK;
    }
    FX_LOGS(ERROR) << "Failed to discover ADB device via service: " << result.error_value();
  }

  return ZX_ERR_NOT_FOUND;
}

zx_status_t AdbClientImpl::ProcessDevice(
    fidl::SyncClient<fuchsia_hardware_usb_device::Device>& device, std::string_view instance) {
  auto desc_result = device->GetDeviceDescriptor();
  if (desc_result.is_error()) {
    FX_LOGS(ERROR) << "GetDeviceDescriptor failed: " << desc_result.error_value().status();
    return desc_result.error_value().status();
  }

  auto config_result = device->GetConfiguration();
  if (config_result.is_error()) {
    FX_LOGS(ERROR) << "GetConfiguration failed: " << config_result.error_value().status();
    return config_result.error_value().status();
  }

  FX_LOGS(INFO) << "Active config: " << static_cast<uint32_t>(config_result->configuration());

  fuchsia_hardware_usb_device::DeviceGetConfigurationDescriptorRequest full_desc_req;
  full_desc_req.config(config_result->configuration());

  auto full_desc_result = device->GetConfigurationDescriptor(full_desc_req);
  if (full_desc_result.is_error()) {
    FX_LOGS(ERROR) << "GetConfigurationDescriptor failed: "
                   << full_desc_result.error_value().status();
    return full_desc_result.error_value().status();
  }
  if (full_desc_result->s() != ZX_OK) {
    FX_LOGS(ERROR) << "GetConfigurationDescriptor status error: " << full_desc_result->s();
    return full_desc_result->s();
  }

  FX_LOGS(INFO) << "Searching for ADB interface in descriptors";
  return FindAdbInterface(instance, full_desc_result->desc().data(),
                          full_desc_result->desc().size());
}

zx_status_t AdbClientImpl::FindAdbInterface(std::string_view instance, const uint8_t* data,
                                            size_t len) {
  usb_desc_iter_t iter;
  if (usb_desc_iter_init_unowned(const_cast<uint8_t*>(data), len, &iter) != ZX_OK) {
    return ZX_ERR_INTERNAL;
  }

  usb_interface_descriptor_t* intf = nullptr;
  while ((intf = usb_desc_iter_next_interface(&iter, true)) != nullptr) {
    // ADB interface: Class 0xFF (Vendor Specific), Subclass 0x42, Protocol 0x01
    if (intf->b_interface_class == 0xFF && intf->b_interface_sub_class == 0x42 &&
        intf->b_interface_protocol == 0x01) {
      FX_LOGS(INFO) << "Found ADB interface";
      usb_endpoint_descriptor_t* ep = nullptr;
      while ((ep = usb_desc_iter_next_endpoint(&iter)) != nullptr) {
        if (usb_ep_type(ep) == fdescriptor::EndpointType::kBulk) {
          if (usb_ep_direction(ep) == USB_ENDPOINT_IN) {
            bulk_in_addr_ = ep->b_endpoint_address;
          } else {
            bulk_out_addr_ = ep->b_endpoint_address;
          }
        }
      }
      if (bulk_in_addr_ && bulk_out_addr_) {
        FX_LOGS(INFO) << "Found ADB bulk endpoints: IN 0x" << std::hex
                      << static_cast<uint32_t>(bulk_in_addr_) << ", OUT 0x"
                      << static_cast<uint32_t>(bulk_out_addr_);
        return ConnectEndpoints(instance);
      }
    }
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t AdbClientImpl::ConnectEndpoints(std::string_view instance) {
  FX_LOGS(INFO) << "Connecting to endpoints for instance: " << instance;
  auto base_svc_dir = component::OpenDirectory("/svc");
  if (base_svc_dir.is_error()) {
    FX_LOGS(ERROR) << "Failed to open /svc: " << base_svc_dir.status_string();
    return base_svc_dir.status_value();
  }

  std::string svc_name = fuchsia_hardware_usb::UsbService::Name;
  std::string svc_path = std::string("/svc/") + svc_name;

  // Try the provided instance first
  std::vector<std::string> instances_to_try;
  instances_to_try.push_back(std::string(instance));

  // Also collect all other instances as fallbacks
  if (std::filesystem::exists(svc_path)) {
    for (const auto& entry : std::filesystem::directory_iterator(svc_path)) {
      std::string name = entry.path().filename().string();
      if (name != instance) {
        instances_to_try.push_back(name);
      }
    }
  }

  // Iterate through available UsbService instances. We expect at least two:
  // 1. One from the usb-bus (parent device). This will likely fail to connect to
  //    endpoints because the usb-composite driver has already bound to it and
  //    claimed the device.
  // 2. One from usb-composite (interface node). This is the one we want to connect
  //    to, as it provides access to the scoped ADB interface endpoints.
  for (const auto& try_instance : instances_to_try) {
    FX_LOGS(INFO) << "Attempting to connect to UsbService instance: " << try_instance;
    auto open_result = component::OpenServiceAt<fuchsia_hardware_usb::UsbService>(
        base_svc_dir->borrow(), try_instance);
    if (open_result.is_error()) {
      continue;
    }

    auto connect_result = open_result->connect_device();
    if (connect_result.is_error()) {
      continue;
    }

    fidl::SyncClient usb{std::move(connect_result.value())};

    auto in_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
    if (in_endpoints.is_error()) {
      continue;
    }
    auto out_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
    if (out_endpoints.is_error()) {
      continue;
    }

    fuchsia_hardware_usb::UsbConnectToEndpointRequest in_req;
    in_req.ep_addr(bulk_in_addr_);
    in_req.ep(std::move(in_endpoints->server));

    auto in_res = usb->ConnectToEndpoint(std::move(in_req));
    if (in_res.is_error()) {
      FX_LOGS(WARNING) << "ConnectToEndpoint (IN) failed for " << try_instance << ": "
                       << in_res.error_value().FormatDescription();
      continue;
    }

    fuchsia_hardware_usb::UsbConnectToEndpointRequest out_req;
    out_req.ep_addr(bulk_out_addr_);
    out_req.ep(std::move(out_endpoints->server));

    auto out_res = usb->ConnectToEndpoint(std::move(out_req));
    if (out_res.is_error()) {
      FX_LOGS(WARNING) << "ConnectToEndpoint (OUT) failed for " << try_instance << ": "
                       << out_res.error_value().FormatDescription();
      continue;
    }

    bulk_in_.Bind(std::move(in_endpoints->client), dispatcher_, this);
    bulk_out_.Bind(std::move(out_endpoints->client), dispatcher_);

    FX_LOGS(INFO) << "Successfully bound ADB endpoints using instance: " << try_instance;
    usb_connected_ = true;
    return ZX_OK;
  }

  FX_LOGS(ERROR) << "Failed to connect to any UsbService instance for ADB endpoints";
  return ZX_ERR_NOT_FOUND;
}

zx_status_t AdbClientImpl::SendPacket(apacket* p) {
  if (!bulk_out_.is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  size_t data_len = sizeof(amessage) + p->msg.data_length;
  std::vector<uint8_t> data(data_len);
  memcpy(data.data(), &p->msg, sizeof(amessage));
  memcpy(data.data() + sizeof(amessage), p->payload.data(), p->msg.data_length);

  fuchsia_hardware_usb_request::Request req;
  req.data().emplace();
  fuchsia_hardware_usb_request::BufferRegion region;
  region.buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::move(data)));
  region.offset(0);
  region.size(data_len);
  req.data()->emplace_back(std::move(region));
  req.defer_completion(false);
  req.information(fuchsia_hardware_usb_request::RequestInfo::WithBulk({}));

  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back(std::move(req));

  fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest queue_request;
  queue_request.req(std::move(requests));

  auto result = bulk_out_->QueueRequests(std::move(queue_request));
  if (result.is_error()) {
    return result.error_value().status();
  }
  return ZX_OK;
}

zx_status_t AdbClientImpl::QueueReadRequest() {
  if (!bulk_in_.is_valid()) {
    FX_LOGS(ERROR) << "Bulk IN endpoint is not valid";
    return ZX_ERR_BAD_STATE;
  }

  fuchsia_hardware_usb_request::Request req;
  req.data().emplace();
  fuchsia_hardware_usb_request::BufferRegion region;
  std::vector<uint8_t> data(1024);
  region.buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::move(data)));
  region.offset(0);
  region.size(1024);
  req.data()->emplace_back(std::move(region));
  req.defer_completion(false);
  req.information(fuchsia_hardware_usb_request::RequestInfo::WithBulk({}));

  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back(std::move(req));

  fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest queue_request;
  queue_request.req(std::move(requests));

  auto result = bulk_in_->QueueRequests(std::move(queue_request));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to queue read request: "
                   << zx_status_get_string(result.error_value().status());
    return result.error_value().status();
  }
  return ZX_OK;
}
