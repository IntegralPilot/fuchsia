// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "allocator.h"

#include <lib/fidl/internal.h>
#include <lib/trace/event.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <zircon/fidl.h>

#include "logical_buffer_collection.h"

namespace sysmem_service {

using Error = fuchsia_sysmem2::Error;

Allocator::Allocator(Sysmem* parent_sysmem)
    : LoggingMixin("allocator"), parent_sysmem_(parent_sysmem) {
  // nothing else to do here
}

Allocator::~Allocator() {}

// static
void Allocator::CreateOwnedV1(fidl::ServerEnd<fuchsia_sysmem::Allocator> server_end, Sysmem* device,
                              fidl::ServerBindingGroup<fuchsia_sysmem::Allocator>& binding_group) {
  auto allocator = std::unique_ptr<Allocator>(new Allocator(device));
  auto v1_server = std::make_unique<V1>(std::move(allocator));
  auto v1_server_ptr = v1_server.get();
  // Ignore the result - allocator will be destroyed and the channel will be closed on error.
  binding_group.AddBinding(device->loop_dispatcher(), std::move(server_end), v1_server_ptr,
                           [v1_server = std::move(v1_server)](fidl::UnbindInfo unbind_info) {
                             // ~v1_server
                           });
}

void Allocator::CreateOwnedV2(fidl::ServerEnd<fuchsia_sysmem2::Allocator> server_end,
                              Sysmem* device,
                              fidl::ServerBindingGroup<fuchsia_sysmem2::Allocator>& binding_group,
                              std::optional<ClientDebugInfo> client_debug_info) {
  auto allocator = std::unique_ptr<Allocator>(new Allocator(device));
  if (client_debug_info.has_value()) {
    allocator->client_debug_info_ = std::move(client_debug_info);
    client_debug_info.reset();
  }
  auto v2_server = std::make_unique<V2>(std::move(allocator));
  auto v2_server_ptr = v2_server.get();

  binding_group.AddBinding(device->loop_dispatcher(), std::move(server_end), v2_server_ptr,
                           [v2_server = std::move(v2_server)](fidl::UnbindInfo unbind_info) {
                             // ~v2_server
                           });
}

template <typename Completer, typename Protocol>
fit::result<std::monostate, fidl::Endpoints<Protocol>> Allocator::CommonAllocateNonSharedCollection(
    Completer& completer) {
  // The AllocateCollection() message skips past the token stage because the
  // client is also the only participant (probably a temp/test client).  Real
  // clients are encouraged to use AllocateSharedCollection() instead, so that
  // the client can share the LogicalBufferCollection with other participants.
  //
  // Because this is a degenerate way to use sysmem, we implement this method
  // in terms of the non-degenerate way.
  //
  // This code is essentially the same as what a client would do if a client
  // wanted to skip the BufferCollectionToken stage without using
  // AllocateCollection().  Essentially, this code is here just so clients
  // that don't need to share their collection don't have to write this code,
  // and can share this code instead.

  // Create a local token.
  zx::result endpoints = fidl::CreateEndpoints<Protocol>();
  if (endpoints.is_error()) {
    LogError(FROM_HERE,
             "Allocator::AllocateCollection() zx::channel::create() failed "
             "- status: %d",
             endpoints.error_value());
    // ~buffer_collection_request
    //
    // Returning an error here causes the sysmem connection to drop also,
    // which seems like a good idea (more likely to recover overall) given
    // the nature of the error.
    completer.Close(ZX_ERR_INTERNAL);
    return fit::error(std::monostate{});
  }

  return fit::success(std::move(endpoints.value()));
}

void Allocator::V1::AllocateNonSharedCollection(
    AllocateNonSharedCollectionRequest& request,
    AllocateNonSharedCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Allocator::AllocateNonSharedCollection");

  fit::result endpoints = allocator_->CommonAllocateNonSharedCollection<
      decltype(completer), fuchsia_sysmem::BufferCollectionToken>(completer);
  if (!endpoints.is_ok()) {
    return;
  }
  auto& [token_client, token_server] = endpoints.value();

  // The server end of the local token goes to Create(), and the client end
  // goes to BindSharedCollection().  The BindSharedCollection() will figure
  // out which token we're talking about based on the koid(s), as usual.
  LogicalBufferCollection::CreateV1(
      std::move(token_server), allocator_->parent_sysmem_,
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);
  LogicalBufferCollection::BindSharedCollection(
      allocator_->parent_sysmem_, token_client.TakeChannel(),
      std::move(request.collection_request()),
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);

  // Now the client can SetConstraints() on the BufferCollection, etc.  The
  // client didn't have to hassle with the BufferCollectionToken, which is the
  // sole upside of the client using this message over
  // AllocateSharedCollection().
}

void Allocator::V2::AllocateNonSharedCollection(
    AllocateNonSharedCollectionRequest& request,
    AllocateNonSharedCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Allocator::AllocateNonSharedCollection");

  if (!request.collection_request().has_value()) {
    allocator_->LogError(FROM_HERE, "AllocateNonSharedCollection requires collection_request set");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  fit::result endpoints = allocator_->CommonAllocateNonSharedCollection<
      decltype(completer), fuchsia_sysmem2::BufferCollectionToken>(completer);
  if (!endpoints.is_ok()) {
    return;
  }
  auto& [token_client, token_server] = endpoints.value();

  // The server end of the local token goes to Create(), and the client end
  // goes to BindSharedCollection().  The BindSharedCollection() will figure
  // out which token we're talking about based on the koid(s), as usual.
  LogicalBufferCollection::CreateV2(
      std::move(token_server), allocator_->parent_sysmem_,
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);
  LogicalBufferCollection::BindSharedCollection(
      allocator_->parent_sysmem_, token_client.TakeChannel(),
      std::move(request.collection_request().value()),
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);

  // Now the client can SetConstraints() on the BufferCollection, etc.  The
  // client didn't have to hassle with the BufferCollectionToken, which is the
  // sole upside of the client using this message over
  // AllocateSharedCollection().
}

void Allocator::V1::AllocateSharedCollection(AllocateSharedCollectionRequest& request,
                                             AllocateSharedCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Allocator::AllocateSharedCollection");

  // The LogicalBufferCollection is self-owned / owned by all the channels it
  // serves.
  //
  // There's no channel served directly by the LogicalBufferCollection.
  // Instead LogicalBufferCollection owns all the FidlServer instances that
  // each own a channel.
  //
  // Initially there's only a channel to the first BufferCollectionToken.  We
  // go ahead and allocate the LogicalBufferCollection here since the
  // LogicalBufferCollection associates all the BufferCollectionToken and
  // BufferCollection bindings to the same LogicalBufferCollection.
  LogicalBufferCollection::CreateV1(
      std::move(request.token_request()), allocator_->parent_sysmem_,
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);
}

void Allocator::V2::AllocateSharedCollection(AllocateSharedCollectionRequest& request,
                                             AllocateSharedCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Allocator::AllocateSharedCollection");

  if (!request.token_request().has_value()) {
    allocator_->LogError(FROM_HERE, "AllocateSharedCollection requires token_request set");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  // The LogicalBufferCollection is self-owned / owned by all the channels it
  // serves.
  //
  // There's no channel served directly by the LogicalBufferCollection.
  // Instead LogicalBufferCollection owns all the FidlServer instances that
  // each own a channel.
  //
  // Initially there's only a channel to the first BufferCollectionToken.  We
  // go ahead and allocate the LogicalBufferCollection here since the
  // LogicalBufferCollection associates all the BufferCollectionToken and
  // BufferCollection bindings to the same LogicalBufferCollection.
  LogicalBufferCollection::CreateV2(
      std::move(request.token_request().value()), allocator_->parent_sysmem_,
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);
}

void Allocator::V1::BindSharedCollection(BindSharedCollectionRequest& request,
                                         BindSharedCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Allocator::BindSharedCollection");

  // The BindSharedCollection() message is about a supposed-to-be-pre-existing
  // logical BufferCollection, but the only association we have to that
  // BufferCollection is the client end of a BufferCollectionToken channel
  // being handed in via token_param.  To find any associated BufferCollection
  // we have to look it up by koid.  The koid table is held by
  // LogicalBufferCollection, so delegate over to LogicalBufferCollection for
  // this request.
  LogicalBufferCollection::BindSharedCollection(
      allocator_->parent_sysmem_, request.token().TakeChannel(),
      std::move(request.buffer_collection_request()),
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);
}

void Allocator::V2::BindSharedCollection(BindSharedCollectionRequest& request,
                                         BindSharedCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Allocator::BindSharedCollection");

  if (!request.token().has_value()) {
    allocator_->LogError(FROM_HERE, "BindSharedCollection requires token set");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  if (!request.buffer_collection_request().has_value()) {
    allocator_->LogError(FROM_HERE, "BindSharedCollection requires buffer_collection_request set");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  // The BindSharedCollection() message is about a supposed-to-be-pre-existing
  // logical BufferCollection, but the only association we have to that
  // BufferCollection is the client end of a BufferCollectionToken channel
  // being handed in via token_param.  To find any associated BufferCollection
  // we have to look it up by koid.  The koid table is held by
  // LogicalBufferCollection, so delegate over to LogicalBufferCollection for
  // this request.
  LogicalBufferCollection::BindSharedCollection(
      allocator_->parent_sysmem_, request.token()->TakeChannel(),
      std::move(request.buffer_collection_request().value()),
      allocator_->client_debug_info_.has_value() ? &*allocator_->client_debug_info_ : nullptr);
}

void Allocator::V1::ValidateBufferCollectionToken(
    ValidateBufferCollectionTokenRequest& request,
    ValidateBufferCollectionTokenCompleter::Sync& completer) {
  zx_status_t status = LogicalBufferCollection::ValidateBufferCollectionToken(
      allocator_->parent_sysmem_, request.token_server_koid());
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_NOT_FOUND);
  completer.Reply(status == ZX_OK);
}

void Allocator::V2::ValidateBufferCollectionToken(
    ValidateBufferCollectionTokenRequest& request,
    ValidateBufferCollectionTokenCompleter::Sync& completer) {
  if (!request.token_server_koid().has_value()) {
    allocator_->LogError(FROM_HERE, "ValidateBufferCollectionToken requires token_server_koid set");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }
  zx_status_t status = LogicalBufferCollection::ValidateBufferCollectionToken(
      allocator_->parent_sysmem_, request.token_server_koid().value());
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_NOT_FOUND);
  fuchsia_sysmem2::AllocatorValidateBufferCollectionTokenResponse response;
  response.is_known().emplace(status == ZX_OK);
  completer.Reply(std::move(response));
}

void Allocator::V1::SetDebugClientInfo(SetDebugClientInfoRequest& request,
                                       SetDebugClientInfoCompleter::Sync& completer) {
  allocator_->client_debug_info_.emplace();
  allocator_->client_debug_info_->name = std::string(request.name().begin(), request.name().end());
  allocator_->client_debug_info_->id = request.id();
}

void Allocator::V2::SetDebugClientInfo(SetDebugClientInfoRequest& request,
                                       SetDebugClientInfoCompleter::Sync& completer) {
  if (!request.name().has_value()) {
    allocator_->LogError(FROM_HERE, "SetDebugClientInfo requires name set");
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }
  uint64_t id = 0;
  if (request.id().has_value()) {
    id = *request.id();
  }
  allocator_->client_debug_info_.emplace();
  allocator_->client_debug_info_->name =
      std::string(request.name()->begin(), request.name()->end());
  allocator_->client_debug_info_->id = id;
}

void Allocator::V1::ConnectToSysmem2Allocator(ConnectToSysmem2AllocatorRequest& request,
                                              ConnectToSysmem2AllocatorCompleter::Sync& completer) {
  std::optional<ClientDebugInfo> client_debug_info_copy = allocator_->client_debug_info_;
  Allocator::CreateOwnedV2(std::move(request.allocator_request()), allocator_->parent_sysmem_,
                           allocator_->parent_sysmem_->v2_allocators(),
                           std::move(client_debug_info_copy));
}

void Allocator::V2::GetVmoInfo(GetVmoInfoRequest& request, GetVmoInfoCompleter::Sync& completer) {
  if (!request.vmo().has_value()) {
    allocator_->LogError(FROM_HERE, "GetVmoInfo requires vmo handle (!has_value)");
    completer.Reply(fit::error(Error::kProtocolDeviation));
    return;
  }
  if (!request.vmo()->is_valid()) {
    allocator_->LogError(FROM_HERE, "GetVmoInfo requires vmo handle (!is_valid)");
    completer.Reply(fit::error(Error::kProtocolDeviation));
    return;
  }
  auto& vmo = *request.vmo();
  auto find_logical_buffer = [this](const zx::vmo& vmo, GetVmoInfoCompleter::Sync& completer)
      -> std::optional<std::pair<Sysmem::FindLogicalBufferByVmoKoidResult, zx_rights_t>> {
    zx_info_handle_basic_t basic_info{};
    zx_status_t status =
        vmo.get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), nullptr, nullptr);
    if (status != ZX_OK) {
      allocator_->LogError(FROM_HERE, "GetVmoInfo couldn't vmo.get_info to get koid");

      Error translated_status;
      if (status == ZX_ERR_ACCESS_DENIED) {
        translated_status = Error::kHandleAccessDenied;
      } else {
        translated_status = Error::kUnspecified;
      }

      completer.Reply(fit::error(translated_status));
      return std::nullopt;
    }
    // Possibly redundant with FIDL generated code.
    if (basic_info.type != ZX_OBJ_TYPE_VMO) {
      allocator_->LogError(FROM_HERE, "GetVmoInfo requires VMO handle");
      completer.Reply(fit::error(Error::kProtocolDeviation));
      return std::nullopt;
    }
    zx_koid_t vmo_koid = basic_info.koid;
    auto logical_buffer_result = allocator_->parent_sysmem_->FindLogicalBufferByVmoKoid(vmo_koid);
    if (!logical_buffer_result.logical_buffer) {
      // We don't log anything in this path because a client may just be checking if a VMO is a
      // sysmem VMO, which could make a LogInfo() here noisy.
      completer.Reply(fit::error(Error::kNotFound));
      return std::nullopt;
    }
    return std::make_pair(logical_buffer_result, basic_info.rights);
  };
  auto maybe_logical_buffer_result = find_logical_buffer(vmo, completer);
  if (!maybe_logical_buffer_result.has_value()) {
    // find_logical_buffer already called completer.Reply
    return;
  }
  auto& [logical_buffer_result, vmo_rights] = *maybe_logical_buffer_result;
  auto& logical_buffer = *logical_buffer_result.logical_buffer;
  fuchsia_sysmem2::AllocatorGetVmoInfoResponse response;
  response.buffer_collection_id() =
      logical_buffer.logical_buffer_collection().buffer_collection_id();
  response.buffer_index() = logical_buffer.buffer_index();
  bool need_weak = request.need_weak().has_value() && *request.need_weak();
  if (need_weak) {
    auto weak_vmo_result = logical_buffer.CreateWeakVmo(allocator_->client_debug_info_.has_value()
                                                            ? *allocator_->client_debug_info_
                                                            : ClientDebugInfo{});
    if (!weak_vmo_result.is_ok()) {
      // This is intentionally not trying to convey a translated zx_status_t, as there's little the
      // client can do to fix any reason why CreateWeakVmo can fail, nor any reason for the client
      // to react differently per zx_status_t value. CreateWeakVmo already logged.
      completer.Reply(fit::error(Error::kUnspecified));
      return;
    }
    auto& maybe_weak_vmo = *weak_vmo_result;
    if (!maybe_weak_vmo.has_value()) {
      completer.Reply(fit::error(Error::kNoMoreStrongVmoHandles));
      return;
    }
    auto& weak_vmo = *maybe_weak_vmo;
    zx_info_handle_basic_t weak_vmo_info{};
    zx_status_t status = weak_vmo.get_info(ZX_INFO_HANDLE_BASIC, &weak_vmo_info,
                                           sizeof(weak_vmo_info), nullptr, nullptr);
    if (status != ZX_OK) {
      allocator_->LogError(FROM_HERE, "GetVmoInfo couldn't weak_vmo.get_info to get rights");
      completer.Reply(fit::error(Error::kUnspecified));
      return;
    }
    zx::vmo attenuated_weak_vmo;
    status = weak_vmo.duplicate(weak_vmo_info.rights & vmo_rights, &attenuated_weak_vmo);
    if (status != ZX_OK) {
      allocator_->LogError(FROM_HERE, "GetVmoInfo duplicate weak_vmo failed: %d", status);
      completer.Reply(fit::error(Error::kUnspecified));
      return;
    }
    response.weak_vmo() = std::move(attenuated_weak_vmo);
  }
  if (logical_buffer_result.is_koid_of_weak_vmo || need_weak) {
    auto dup_result = logical_buffer.logical_buffer_collection().DupCloseWeakAsapClientEnd(
        logical_buffer.buffer_index());
    if (dup_result.is_error()) {
      completer.Reply(fit::error{Error::kUnspecified});
      return;
    }
    response.close_weak_asap() = std::move(dup_result).value();
  }
  bool need_single_buffer_settings =
      request.need_single_buffer_settings().has_value() && *request.need_single_buffer_settings();
  if (need_single_buffer_settings) {
    // intentional copy/clone
    response.single_buffer_settings() =
        logical_buffer.logical_buffer_collection().single_buffer_settings();
  }
  if (request.constraints_to_check().has_value()) {
    response.constraints_ok() =
        logical_buffer.logical_buffer_collection().CheckConstraintsAgainstExistingSettings(
            *request.constraints_to_check());
  }
  if (request.vmo_settings_to_check().has_value()) {
    auto maybe_other_logical_buffer_result =
        find_logical_buffer(*request.vmo_settings_to_check(), completer);
    if (!maybe_other_logical_buffer_result.has_value()) {
      // find_logical_buffer already called completer.Reply
      return;
    }
    auto& other_logical_buffer = *maybe_other_logical_buffer_result->first.logical_buffer;
    if (request.vmo_settings_to_check_ignore_size().has_value() &&
        *request.vmo_settings_to_check_ignore_size()) {
      // this path can be useful for video decoder input buffers where input buffer sizes may vary
      // when buffers are from separate BufferCollection(s), but other buffer properties don't vary
      //
      // copy/clone, un-set size fields, compare; clients typically do GetVmoInfo up to once per
      // buffer and cache the result; the clone here isn't likely to be a problem
      auto other = other_logical_buffer.logical_buffer_collection().single_buffer_settings();
      auto this_one = logical_buffer.logical_buffer_collection().single_buffer_settings();
      other.buffer_settings()->size_bytes().reset();
      other.buffer_settings()->raw_vmo_size().reset();
      this_one.buffer_settings()->size_bytes().reset();
      this_one.buffer_settings()->raw_vmo_size().reset();
      response.vmo_settings_match() = (other == this_one);
    } else {
      // compare in place, including the size fields
      response.vmo_settings_match() =
          (other_logical_buffer.logical_buffer_collection().single_buffer_settings() ==
           logical_buffer.logical_buffer_collection().single_buffer_settings());
    }
  }

  completer.Reply(fit::ok(std::move(response)));
}

void Allocator::V2::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_sysmem2::Allocator> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  allocator_->LogError(FROM_HERE, "Allocator unknown method - ordinal: %" PRIx64,
                       metadata.method_ordinal);
  completer.Close(ZX_ERR_INTERNAL);
}

}  // namespace sysmem_service
