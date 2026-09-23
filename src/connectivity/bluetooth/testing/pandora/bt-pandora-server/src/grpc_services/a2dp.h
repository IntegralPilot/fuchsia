// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_GRPC_SERVICES_A2DP_H_
#define SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_GRPC_SERVICES_A2DP_H_

#include <fidl/fuchsia.bluetooth.a2dp/cpp/fidl.h>

#include "lib/fidl/cpp/wire/internal/transport.h"
#include "third_party/github.com/google/bt-test-interfaces/src/pandora/a2dp.grpc.pb.h"

class A2dpService : public pandora::A2DP::Service {
 public:
  explicit A2dpService(async_dispatcher_t* dispatcher);

  ::grpc::Status OpenSource(::grpc::ServerContext* context,
                            const ::pandora::OpenSourceRequest* request,
                            ::pandora::OpenSourceResponse* response) override;

  ::grpc::Status OpenSink(::grpc::ServerContext* context, const ::pandora::OpenSinkRequest* request,
                          ::pandora::OpenSinkResponse* response) override;

  ::grpc::Status WaitSource(::grpc::ServerContext* context,
                            const ::pandora::WaitSourceRequest* request,
                            ::pandora::WaitSourceResponse* response) override;

  ::grpc::Status WaitSink(::grpc::ServerContext* context, const ::pandora::WaitSinkRequest* request,
                          ::pandora::WaitSinkResponse* response) override;

  ::grpc::Status IsSuspended(::grpc::ServerContext* context,
                             const ::pandora::IsSuspendedRequest* request,
                             ::google::protobuf::BoolValue* response) override;

  ::grpc::Status Start(::grpc::ServerContext* context, const ::pandora::StartRequest* request,
                       ::pandora::StartResponse* response) override;

  ::grpc::Status Suspend(::grpc::ServerContext* context, const ::pandora::SuspendRequest* request,
                         ::pandora::SuspendResponse* response) override;

  ::grpc::Status Close(::grpc::ServerContext* context, const ::pandora::CloseRequest* request,
                       ::pandora::CloseResponse* response) override;

  ::grpc::Status GetAudioEncoding(::grpc::ServerContext* context,
                                  const ::pandora::GetAudioEncodingRequest* request,
                                  ::pandora::GetAudioEncodingResponse* response) override;

  ::grpc::Status PlaybackAudio(::grpc::ServerContext* context,
                               ::grpc::ServerReader<::pandora::PlaybackAudioRequest>* reader,
                               ::pandora::PlaybackAudioResponse* response) override;

  ::grpc::Status CaptureAudio(
      ::grpc::ServerContext* context, const ::pandora::CaptureAudioRequest* request,
      ::grpc::ServerWriter<::pandora::CaptureAudioResponse>* writer) override;

 private:
  fidl::SyncClient<fuchsia_bluetooth_a2dp::AudioMode> audio_mode_client_;
};

#endif  // SRC_CONNECTIVITY_BLUETOOTH_TESTING_PANDORA_BT_PANDORA_SERVER_SRC_GRPC_SERVICES_A2DP_H_
