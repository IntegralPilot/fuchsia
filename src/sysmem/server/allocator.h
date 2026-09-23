// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_ALLOCATOR_H_
#define SRC_SYSMEM_SERVER_ALLOCATOR_H_

#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/zx/channel.h>

#include "logging.h"
#include "logical_buffer_collection.h"
#include "sysmem.h"

namespace sysmem_service {

// An instance of this class serves an Allocator connection.  The lifetime of
// the instance is 1:1 with the Allocator channel.
//
// Because Allocator is essentially self-contained and handling the server end
// of a channel, most of Allocator is private.
class Allocator : public LoggingMixin {
 public:
  // Public for std::unique_ptr<Allocator>:
  ~Allocator();

  // Create a V1 allocator owned by the binding_group.
  static void CreateOwnedV1(fidl::ServerEnd<fuchsia_sysmem::Allocator> server_end, Sysmem* device,
                            fidl::ServerBindingGroup<fuchsia_sysmem::Allocator>& binding_group);

  // Create a v2 allocator owned by the binding_group.
  //
  // The returned reference may be deallocated as soon as the current thread returns to the FIDL
  // dispatcher, but can be used until then. The return value can be removed once we've deleted
  // fuchsia_sysmem::Allocator and ConnectToSysmem2Allocator.
  static void CreateOwnedV2(fidl::ServerEnd<fuchsia_sysmem2::Allocator> server_end, Sysmem* device,
                            fidl::ServerBindingGroup<fuchsia_sysmem2::Allocator>& binding_group,
                            std::optional<ClientDebugInfo> client_debug_info = std::nullopt);

 private:
  struct V1;
  struct V2;

  Allocator(Sysmem* parent_device);

  template <typename Completer, typename Protocol>
  fit::result<std::monostate, fidl::Endpoints<Protocol>> CommonAllocateNonSharedCollection(
      Completer& completer);

  Sysmem* parent_sysmem_ = nullptr;

  std::optional<ClientDebugInfo> client_debug_info_;
};

struct Allocator::V1 : public fidl::Server<fuchsia_sysmem::Allocator> {
  explicit V1(std::unique_ptr<Allocator> allocator) : allocator_(std::move(allocator)) {}

  void AllocateNonSharedCollection(AllocateNonSharedCollectionRequest& request,
                                   AllocateNonSharedCollectionCompleter::Sync& completer) override;
  void AllocateSharedCollection(AllocateSharedCollectionRequest& request,
                                AllocateSharedCollectionCompleter::Sync& completer) override;
  void BindSharedCollection(BindSharedCollectionRequest& request,
                            BindSharedCollectionCompleter::Sync& completer) override;
  void ValidateBufferCollectionToken(
      ValidateBufferCollectionTokenRequest& request,
      ValidateBufferCollectionTokenCompleter::Sync& completer) override;
  void SetDebugClientInfo(SetDebugClientInfoRequest& request,
                          SetDebugClientInfoCompleter::Sync& completer) override;
  void ConnectToSysmem2Allocator(ConnectToSysmem2AllocatorRequest& request,
                                 ConnectToSysmem2AllocatorCompleter::Sync& completer) override;

  std::unique_ptr<Allocator> allocator_;
};

struct Allocator::V2 : public fidl::Server<fuchsia_sysmem2::Allocator> {
  explicit V2(std::unique_ptr<Allocator> allocator) : allocator_(std::move(allocator)) {}

  void AllocateNonSharedCollection(AllocateNonSharedCollectionRequest& request,
                                   AllocateNonSharedCollectionCompleter::Sync& completer) override;
  void AllocateSharedCollection(AllocateSharedCollectionRequest& request,
                                AllocateSharedCollectionCompleter::Sync& completer) override;
  void BindSharedCollection(BindSharedCollectionRequest& request,
                            BindSharedCollectionCompleter::Sync& completer) override;
  void ValidateBufferCollectionToken(
      ValidateBufferCollectionTokenRequest& request,
      ValidateBufferCollectionTokenCompleter::Sync& completer) override;
  void SetDebugClientInfo(SetDebugClientInfoRequest& request,
                          SetDebugClientInfoCompleter::Sync& completer) override;
  void GetVmoInfo(GetVmoInfoRequest& request, GetVmoInfoCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_sysmem2::Allocator> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  std::unique_ptr<Allocator> allocator_;
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_ALLOCATOR_H_
