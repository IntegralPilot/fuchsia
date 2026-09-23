// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_GRPC_SERVICES_GATT_H_
#define SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_GRPC_SERVICES_GATT_H_

#include <fidl/fuchsia.bluetooth.affordances/cpp/fidl.h>

#include "third_party/github.com/google/bt-test-interfaces/src/pandora/gatt.grpc.pb.h"

class GattService : public pandora::GATT::Service {
 public:
  explicit GattService(async_dispatcher_t* dispatcher);

  ::grpc::Status ExchangeMTU(::grpc::ServerContext* context,
                             const ::pandora::ExchangeMTURequest* request,
                             ::pandora::ExchangeMTUResponse* response) override;

  ::grpc::Status WriteAttFromHandle(::grpc::ServerContext* context,
                                    const ::pandora::WriteRequest* request,
                                    ::pandora::WriteResponse* response) override;

  ::grpc::Status DiscoverServiceByUuid(::grpc::ServerContext* context,
                                       const ::pandora::DiscoverServiceByUuidRequest* request,
                                       ::pandora::DiscoverServicesResponse* response) override;

  ::grpc::Status DiscoverServices(::grpc::ServerContext* context,
                                  const ::pandora::DiscoverServicesRequest* request,
                                  ::pandora::DiscoverServicesResponse* response) override;

  ::grpc::Status DiscoverServicesSdp(::grpc::ServerContext* context,
                                     const ::pandora::DiscoverServicesSdpRequest* request,
                                     ::pandora::DiscoverServicesSdpResponse* response) override;

  ::grpc::Status ClearCache(::grpc::ServerContext* context,
                            const ::pandora::ClearCacheRequest* request,
                            ::pandora::ClearCacheResponse* response) override;

  ::grpc::Status ReadCharacteristicFromHandle(
      ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicRequest* request,
      ::pandora::ReadCharacteristicResponse* response) override;

  // TODO(https://fxbug.dev/450959787): Upstream this RPC. The above `ReadCharacteristicFromHandle`
  // request does not include the GATT service handle, which is needed to dispatch a read req on
  // the right RemoteService when using the Sapphire gatt2 API. We have locally added this new RPC
  // which supplies both handles and used it in mmi2grpc in tandem with `DiscoverServices`.
  ::grpc::Status ReadCharacteristic(::grpc::ServerContext* context,
                                    const ::pandora::ReadCharacteristicWithServiceRequest* request,
                                    ::pandora::ReadCharacteristicResponse* response) override;

  ::grpc::Status ReadCharacteristicsFromUuid(
      ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicsFromUuidRequest* request,
      ::pandora::ReadCharacteristicsFromUuidResponse* response) override;

  ::grpc::Status ReadCharacteristicDescriptorFromHandle(
      ::grpc::ServerContext* context, const ::pandora::ReadCharacteristicDescriptorRequest* request,
      ::pandora::ReadCharacteristicDescriptorResponse* response) override;

  // Only one service can be registered at a time without rebooting.
  ::grpc::Status RegisterService(::grpc::ServerContext* context,
                                 const ::pandora::RegisterServiceRequest* request,
                                 ::pandora::RegisterServiceResponse* response) override;

  ::grpc::Status SetCharacteristicNotificationFromHandle(
      ::grpc::ServerContext* context,
      const ::pandora::SetCharacteristicNotificationFromHandleRequest* request,
      ::pandora::SetCharacteristicNotificationFromHandleResponse* response) override;

  ::grpc::Status WaitCharacteristicNotification(
      ::grpc::ServerContext* context,
      const ::pandora::WaitCharacteristicNotificationRequest* request,
      ::pandora::WaitCharacteristicNotificationResponse* response) override;

  ::grpc::Status NotifyOnCharacteristic(
      ::grpc::ServerContext* context, const ::pandora::NotifyOnCharacteristicRequest* request,
      ::pandora::NotifyOnCharacteristicResponse* response) override;

  ::grpc::Status IndicateOnCharacteristic(
      ::grpc::ServerContext* context, const ::pandora::IndicateOnCharacteristicRequest* request,
      ::pandora::IndicateOnCharacteristicResponse* response) override;

 private:
  fidl::SyncClient<fuchsia_bluetooth_affordances::GattClientController>
      gatt_client_controller_client_;
};

#endif  // SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_GRPC_SERVICES_GATT_H_
