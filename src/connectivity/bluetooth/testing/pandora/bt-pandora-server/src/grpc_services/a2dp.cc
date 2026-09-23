// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "a2dp.h"

#include <lib/syslog/cpp/macros.h>

#include "fidl/fuchsia.bluetooth.a2dp/cpp/markers.h"
#include "lib/component/incoming/cpp/protocol.h"

using grpc::Status;

A2dpService::A2dpService(async_dispatcher_t* dispatcher) {
  // Connec to fuchsia.bluetooth.a2dp.AudioMode
  zx::result audio_mode_client_end = component::Connect<fuchsia_bluetooth_a2dp::AudioMode>();
  if (!audio_mode_client_end.is_ok()) {
    FX_LOGS(ERROR) << "Error connecting to AudioMode service: "
                   << audio_mode_client_end.error_value();
    return;
  }
  audio_mode_client_.Bind(std::move(*audio_mode_client_end));
}

// TODO(https://fxbug.dev/316721276): Implement gRPCs necessary to enable GAP/A2DP testing.

Status A2dpService::OpenSource(::grpc::ServerContext* context,
                               const ::pandora::OpenSourceRequest* request,
                               ::pandora::OpenSourceResponse* response) {
  return {/*OK*/};
}

Status A2dpService::OpenSink(::grpc::ServerContext* context,
                             const ::pandora::OpenSinkRequest* request,
                             ::pandora::OpenSinkResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::WaitSource(::grpc::ServerContext* context,
                               const ::pandora::WaitSourceRequest* request,
                               ::pandora::WaitSourceResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::WaitSink(::grpc::ServerContext* context,
                             const ::pandora::WaitSinkRequest* request,
                             ::pandora::WaitSinkResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::IsSuspended(::grpc::ServerContext* context,
                                const ::pandora::IsSuspendedRequest* request,
                                ::google::protobuf::BoolValue* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::Start(::grpc::ServerContext* context, const ::pandora::StartRequest* request,
                          ::pandora::StartResponse* response) {
  return {/*OK*/};
}

Status A2dpService::Suspend(::grpc::ServerContext* context,
                            const ::pandora::SuspendRequest* request,
                            ::pandora::SuspendResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::Close(::grpc::ServerContext* context, const ::pandora::CloseRequest* request,
                          ::pandora::CloseResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::GetAudioEncoding(::grpc::ServerContext* context,
                                     const ::pandora::GetAudioEncodingRequest* request,
                                     ::pandora::GetAudioEncodingResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::PlaybackAudio(::grpc::ServerContext* context,
                                  ::grpc::ServerReader<::pandora::PlaybackAudioRequest>* reader,
                                  ::pandora::PlaybackAudioResponse* response) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}

Status A2dpService::CaptureAudio(::grpc::ServerContext* context,
                                 const ::pandora::CaptureAudioRequest* request,
                                 ::grpc::ServerWriter<::pandora::CaptureAudioResponse>* writer) {
  return Status(grpc::StatusCode::UNIMPLEMENTED, "");
}
