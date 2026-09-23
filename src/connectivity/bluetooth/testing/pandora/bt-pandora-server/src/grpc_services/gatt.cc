// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gatt.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include "src/connectivity/bluetooth/testing/bt-affordances/ffi_c/bindings.h"

using grpc::Status;
using grpc::StatusCode;

GattService::GattService(async_dispatcher_t* dispatcher) {
  zx::result client_end_client =
      component::Connect<fuchsia_bluetooth_affordances::GattClientController>();
  if (client_end_client.is_ok()) {
    gatt_client_controller_client_.Bind(std::move(*client_end_client));
  } else {
    FX_LOGS(ERROR) << "Error connecting to GattClientController service: "
                   << client_end_client.status_string();
  }
}

Status GattService::ExchangeMTU(::grpc::ServerContext* context,
                                const ::pandora::ExchangeMTURequest* request,
                                ::pandora::ExchangeMTUResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::WriteAttFromHandle(::grpc::ServerContext* context,
                                       const ::pandora::WriteRequest* request,
                                       ::pandora::WriteResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::DiscoverServiceByUuid(::grpc::ServerContext* context,
                                          const ::pandora::DiscoverServiceByUuidRequest* request,
                                          ::pandora::DiscoverServicesResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::DiscoverServices(::grpc::ServerContext* context,
                                     const ::pandora::DiscoverServicesRequest* request,
                                     ::pandora::DiscoverServicesResponse* response) {
  auto result = gatt_client_controller_client_->DiscoverServices();
  if (result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.GattClientController/DiscoverServices error: " +
                      result.error_value().FormatDescription());
  }

  for (const fuchsia_bluetooth_gatt2::ServiceInfo& service : *result->services()) {
    pandora::GattService* new_service = response->add_services();

    new_service->set_handle(service.handle()->value());

    UuidBytes ffi_uuid;
    std::ranges::copy(service.type()->value(), ffi_uuid.value);
    char uuid_str[37];
    uuid_to_string(ffi_uuid, uuid_str);
    new_service->set_uuid(uuid_str);

    if (service.characteristics().has_value()) {
      for (const fuchsia_bluetooth_gatt2::Characteristic& characteristic :
           *service.characteristics()) {
        new_service->add_characteristics()->set_handle(characteristic.handle()->value());
      }
    }
  }

  return Status::OK;
}

Status GattService::DiscoverServicesSdp(::grpc::ServerContext* context,
                                        const ::pandora::DiscoverServicesSdpRequest* request,
                                        ::pandora::DiscoverServicesSdpResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::ClearCache(::grpc::ServerContext* context,
                               const ::pandora::ClearCacheRequest* request,
                               ::pandora::ClearCacheResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::ReadCharacteristicFromHandle(
    ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicRequest* request,
    ::pandora::ReadCharacteristicResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::ReadCharacteristic(
    ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicWithServiceRequest* request,
    ::pandora::ReadCharacteristicResponse* response) {
  struct ReadCharacteristicResult result;
  if (read_characteristic(request->service_handle(), request->characteristic_handle(), &result) !=
      ZX_OK) {
    return Status(StatusCode::INTERNAL, "Error in Rust affordances (check logs)");
  }

  response->mutable_value()->set_handle(result.handle);
  response->mutable_value()->set_value(result.value, result.value_len);

  return {/*OK*/};
}

Status GattService::ReadCharacteristicsFromUuid(
    ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicsFromUuidRequest* request,
    ::pandora::ReadCharacteristicsFromUuidResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::ReadCharacteristicDescriptorFromHandle(
    ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicDescriptorRequest* request,
    ::pandora::ReadCharacteristicDescriptorResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::RegisterService(::grpc::ServerContext* context,
                                    const ::pandora::RegisterServiceRequest* request,
                                    ::pandora::RegisterServiceResponse* response) {
  pandora::GattCharacteristicParams characteristic = request->service().characteristics()[0];

  // Use arbitrary handles.
  if (publish_service(
          /*handle=*/0x123, /*uuid=*/request->service().uuid().c_str(),
          /*characteristic_handle=*/0x456,
          /*characteristic_uuid=*/characteristic.uuid().c_str(),
          /*characteristic_properties=*/static_cast<uint16_t>(characteristic.properties()),
          /*characteristic_permissions=*/static_cast<uint16_t>(characteristic.permissions())) !=
      ZX_OK) {
    return Status(StatusCode::INTERNAL, "Error in Rust affordances (check logs)");
  }

  // TODO(https://fxbug.dev/42075291): Expose attribute handles for testing.
  //
  // PTS asks for attribute handles. Sapphire assigns them deterministically, so we can rely on the
  // value 0xe being assigned here for now, but this is not a safe assumption in the long term. We
  // should modify the API or create a new API that exposes attribute handles for testing.
  response->mutable_service()->add_characteristics()->set_handle(0xe);

  return {/*OK*/};
}

Status GattService::SetCharacteristicNotificationFromHandle(
    ::grpc::ServerContext* context,
    const ::pandora::SetCharacteristicNotificationFromHandleRequest* request,
    ::pandora::SetCharacteristicNotificationFromHandleResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::WaitCharacteristicNotification(
    ::grpc::ServerContext* context, const ::pandora::WaitCharacteristicNotificationRequest* request,
    ::pandora::WaitCharacteristicNotificationResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status GattService::NotifyOnCharacteristic(::grpc::ServerContext* context,
                                           const ::pandora::NotifyOnCharacteristicRequest* request,
                                           ::pandora::NotifyOnCharacteristicResponse* response) {
  // TODO(https://fxbug.dev/467722411): fuchsia.bluetooth.gatt2 uses a "ServiceHandle" and
  // "CharacteristicHandle" to identify ATT characteristics. We need either (1) a mode that is
  // enabled for PTS that does not munge ATT Handles, or (2) a test-only FIDL protocol that resolves
  // ATT Handles to (ServiceHandle, CharacteristicHandle).
  return Status(StatusCode::UNIMPLEMENTED, "Todo. See https://fxbug.dev/467722411.");

  uint64_t service_handle = 0;
  uint64_t characteristic_handle = 0;

  if (register_characteristic_notifier(service_handle, characteristic_handle) != ZX_OK) {
    return Status(StatusCode::INTERNAL, "Error in Rust affordances (check logs)");
  }

  return {/*OK*/};
}

Status GattService::IndicateOnCharacteristic(
    ::grpc::ServerContext* context, const ::pandora::IndicateOnCharacteristicRequest* request,
    ::pandora::IndicateOnCharacteristicResponse* response) {
  // TODO(https://fxbug.dev/467722411): See above `NotifyOnCharacteristic`.
  return Status(StatusCode::UNIMPLEMENTED, "Todo. See https://fxbug.dev/467722411.");

  uint64_t service_handle = 0;
  uint64_t characteristic_handle = 0;

  if (register_characteristic_notifier(service_handle, characteristic_handle) != ZX_OK) {
    return Status(StatusCode::INTERNAL, "Error in Rust affordances (check logs)");
  }

  return {/*OK*/};
}
