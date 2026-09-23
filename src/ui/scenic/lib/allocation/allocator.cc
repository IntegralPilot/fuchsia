// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/allocation/allocator.h"

#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <memory>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"

using allocation::BufferCollectionUsage;
using fuchsia_ui_composition::RegisterBufferCollectionError;
using fuchsia_ui_composition::RegisterBufferCollectionUsages;

namespace allocation {

namespace {

RegisterBufferCollectionUsages UsageToUsages(
    fuchsia_ui_composition::RegisterBufferCollectionUsage usage) {
  switch (usage) {
    case fuchsia_ui_composition::RegisterBufferCollectionUsage::kDefault:
      return RegisterBufferCollectionUsages::kDefault;
    case fuchsia_ui_composition::RegisterBufferCollectionUsage::kScreenshot:
      return RegisterBufferCollectionUsages::kScreenshot;
  }
}

}  // namespace

Allocator::Allocator(async_dispatcher_t* dispatcher, sys::ComponentContext* app_context,
                     const std::vector<std::shared_ptr<BufferCollectionImporter>>&
                         default_buffer_collection_importers,
                     const std::vector<std::shared_ptr<BufferCollectionImporter>>&
                         screenshot_buffer_collection_importers,
                     fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator,
                     inspect::Node inspect_node)
    : default_buffer_collection_importers_(default_buffer_collection_importers),
      screenshot_buffer_collection_importers_(screenshot_buffer_collection_importers),
      sysmem_allocator_(std::move(sysmem_allocator)),
      inspect_node_(std::move(inspect_node)),
      weak_factory_(this),
      executor_(dispatcher) {
  FX_DCHECK(app_context);
  app_context->outgoing()->AddProtocol<fuchsia_ui_composition::Allocator>(
      bindings_.CreateHandler(this, executor_.dispatcher(), fidl::kIgnoreBindingClosure));

  inspect_registered_buffer_collections_ =
      inspect_node_.CreateUint("Registered Buffer Collections", 0);
  inspect_released_buffer_collections_ = inspect_node_.CreateUint("Released Buffer Collections", 0);
  inspect_failed_buffer_collections_ = inspect_node_.CreateUint("Failed Buffer Collections", 0);
  inspect_outstanding_buffer_collections_ =
      inspect_node_.CreateUint("Outstanding Buffer Collections", 0);
}

Allocator::~Allocator() {
  FX_DCHECK(executor_.dispatcher() == async_get_default_dispatcher());

  // Allocator outlives |*_buffer_collection_importers_| instances, because we hold shared_ptrs. It
  // is safe to release all remaining buffer collections because there should be no more usage.
  while (!buffer_collections_.empty()) {
    ReleaseBufferCollection(buffer_collections_.begin()->first);
  }
}

std::optional<Allocator::ParsedArgs> Allocator::ParseArgs(
    fuchsia_ui_composition::RegisterBufferCollectionArgs args) {
  // It's okay if there's no specified RegisterBufferCollectionUsage. In that case, assume it is
  // DEFAULT.
  if (!(args.buffer_collection_token().has_value() ||
        args.buffer_collection_token2().has_value()) ||
      !args.export_token().has_value()) {
    FX_LOGS(ERROR) << "RegisterBufferCollection called with missing arguments";
    return std::nullopt;
  }

  if (args.buffer_collection_token().has_value() && !args.buffer_collection_token()->is_valid()) {
    FX_LOGS(ERROR) << "RegisterBufferCollection called with invalid buffer_collection_token";
    return std::nullopt;
  }

  if (args.buffer_collection_token2().has_value() && !args.buffer_collection_token2()->is_valid()) {
    FX_LOGS(ERROR) << "RegisterBufferCollection called with invalid buffer_collection_token2";
    return std::nullopt;
  }

  if (args.buffer_collection_token().has_value() && args.buffer_collection_token2().has_value()) {
    FX_LOGS(ERROR)
        << "RegisterBufferCollection called with both buffer_collection_token and buffer_collection_token2 set. Exactly one must be set.";
    return std::nullopt;
  }
  // Exactly one set.
  FX_DCHECK(!!args.buffer_collection_token().has_value() ^
            !!args.buffer_collection_token2().has_value());

  if (!args.export_token()->value().is_valid()) {
    FX_LOGS(ERROR) << "RegisterBufferCollection called with invalid export token";
    return std::nullopt;
  }

  // Check if there is a valid peer.
  if (fsl::GetRelatedKoid(args.export_token()->value().get()) == ZX_KOID_INVALID) {
    FX_LOGS(ERROR) << "RegisterBufferCollection called with no valid import tokens";
    return std::nullopt;
  }

  if (args.usages().has_value() && args.usages()->has_unknown_bits()) {
    FX_LOGS(ERROR) << "Arguments contain unknown BufferCollectionUsage type";
    return std::nullopt;
  }

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token;
  if (args.buffer_collection_token2().has_value()) {
    token = std::move(args.buffer_collection_token2().value());
  } else {
    token = fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(
        args.buffer_collection_token()->TakeChannel());
  }

  // Grab object koid to be used as unique_id.
  const GlobalBufferCollectionId koid = fsl::GetKoid(args.export_token()->value().get());
  FX_DCHECK(koid != ZX_KOID_INVALID);

  // If no usages are set we default to DEFAULT. Otherwise the newer "usages" value takes precedence
  // over the deprecated "usage" variant.
  RegisterBufferCollectionUsages buffer_collection_usages =
      RegisterBufferCollectionUsages::kDefault;
  if (args.usages().has_value()) {
    buffer_collection_usages = args.usages().value();
  } else if (args.usage().has_value()) {
    buffer_collection_usages = UsageToUsages(args.usage().value());
  }

  return ParsedArgs{
      .koid = koid,
      .buffer_collection_usages = buffer_collection_usages,
      .export_token = std::move(args.export_token().value()),
      .buffer_collection_token = std::move(token),
  };
}

void Allocator::RegisterBufferCollection(RegisterBufferCollectionRequest& request,
                                         RegisterBufferCollectionCompleter::Sync& completer) {
  RegisterBufferCollection(
      std::move(request.args()),
      [completer = completer.ToAsync()](auto result) mutable { completer.Reply(result); });
}

void Allocator::RegisterBufferCollection(
    fuchsia_ui_composition::RegisterBufferCollectionArgs args,
    fit::function<void(fit::result<RegisterBufferCollectionError>)> completer) {
  FX_DCHECK(executor_.dispatcher() == async_get_default_dispatcher());

  IncrementRegisteredBufferCollections();

  auto parsed_args = ParseArgs(std::move(args));
  if (!parsed_args) {
    FX_LOGS(ERROR) << "RegisterBufferCollection failed to parse args";
    IncrementFailedBufferCollections();
    completer(fit::error(RegisterBufferCollectionError::kBadOperation));
    return;
  }
  TRACE_DURATION("gfx", "allocation::Allocator::RegisterBufferCollection[begin]");
  TRACE_FLOW_BEGIN("gfx", "allocation::Allocator::RegisterBufferCollection", parsed_args->koid);

  // Check if this export token has already been used.
  if (buffer_collections_.find(parsed_args->koid) != buffer_collections_.end()) {
    FX_LOGS(ERROR) << "RegisterBufferCollection called with pre-registered export token";
    IncrementFailedBufferCollections();
    completer(fit::error(RegisterBufferCollectionError::kBadOperation));
    return;
  }

  // Check if the buffer collection token is valid.
  fidl::Arena arena;
  sysmem_allocator_
      ->ValidateBufferCollectionToken(
          fuchsia_sysmem2::wire::AllocatorValidateBufferCollectionTokenRequest::Builder(arena)
              .token_server_koid(
                  fsl::GetRelatedKoid(parsed_args->buffer_collection_token.channel().get()))
              .Build())
      // TODO(https://fxbug.dev/502763366): Scenic assumes immortality of Allocator.
      .ThenExactlyOnce([this, parsed_args = std::move(*parsed_args),
                        completer = std::move(completer)](auto& result) mutable {
        if (!result.ok() || !result->has_is_known() || !result->is_known()) {
          FX_LOGS(ERROR) << "RegisterBufferCollection called with a buffer collection token where "
                            "ValidateBufferCollectionToken() failed";
          IncrementFailedBufferCollections();
          completer(fit::error(RegisterBufferCollectionError::kBadOperation));
          return;
        }
        // Create a token for each of the buffer collection importers.
        DuplicateBufferCollectionToken(std::move(parsed_args), std::move(completer));
      });
}

Allocator::Importers Allocator::GetImporters(const RegisterBufferCollectionUsages usages) const {
  Importers importers;
  if (usages & RegisterBufferCollectionUsages::kDefault) {
    for (const auto& importer : default_buffer_collection_importers_) {
      importers.emplace_back(*importer, BufferCollectionUsage::kClientImage);
    }
  }
  if (usages & RegisterBufferCollectionUsages::kScreenshot) {
    for (const auto& importer : screenshot_buffer_collection_importers_) {
      importers.emplace_back(*importer, BufferCollectionUsage::kRenderTarget);
    }
  }

  return importers;
}

void Allocator::DuplicateBufferCollectionToken(ParsedArgs parsed_args, Completer completer) {
  auto& [koid, buffer_collection_usages, export_token, buffer_collection_token] = parsed_args;
  TRACE_DURATION("gfx", "allocation::Allocator::RegisterBufferCollection[duplicate]");
  TRACE_FLOW_STEP("gfx", "allocation::Allocator::RegisterBufferCollection", koid);
  auto importers = GetImporters(buffer_collection_usages);
  FX_DCHECK(importers.size() > 0);

  fidl::WireClient<fuchsia_sysmem2::BufferCollectionToken> token{std::move(buffer_collection_token),
                                                                 async_get_default_dispatcher()};
  fidl::Arena arena;
  std::vector<zx_rights_t> rights_attenuation_masks(importers.size() - 1, ZX_RIGHT_SAME_RIGHTS);
  token
      ->DuplicateSync(
          fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncRequest::Builder(arena)
              .rights_attenuation_masks(
                  fidl::VectorView<zx_rights_t>::FromExternal(rights_attenuation_masks))
              .Build())
      // TODO(https://fxbug.dev/502763366): Scenic assumes immortality of Allocator.
      .ThenExactlyOnce([this, token = std::move(token), parsed_args = std::move(parsed_args),
                        importers = std::move(importers),
                        completer = std::move(completer)](auto& result) mutable {
        if (!result.ok()) {
          FX_LOGS(ERROR) << "RegisterBufferCollection called with a buffer collection token where "
                            "Duplicate() failed";
          IncrementFailedBufferCollections();
          completer(fit::error(RegisterBufferCollectionError::kBadOperation));
          return;
        }
        // Sysmem always fills out tokens vector (can be 0 length if we passed 0-length
        // rights_attenuation_masks above)
        FX_DCHECK(result->has_tokens());
        BufferCollectionTokens tokens;
        tokens.reserve(importers.size());
        tokens.push_back(*token.UnbindMaybeGetEndpoint());
        std::ranges::move(result->tokens(), std::back_inserter(tokens));
        ImportBufferCollection(std::move(parsed_args), std::move(importers), std::move(tokens),
                               std::move(completer));
      });
}

void Allocator::ImportBufferCollection(ParsedArgs parsed_args, Importers importers,
                                       BufferCollectionTokens tokens, Completer completer) {
  auto& [koid, buffer_collection_usages, export_token, _] = parsed_args;
  TRACE_DURATION("gfx", "allocation::Allocator::RegisterBufferCollection[import]");
  TRACE_FLOW_STEP("gfx", "allocation::Allocator::RegisterBufferCollection", koid);

  // Loop over each of the importers and provide each of them with a token from the vector we
  // created above.
  std::vector<fpromise::promise<>> promises;
  promises.reserve(importers.size());
  for (uint32_t i = 0; i < importers.size(); i++) {
    auto& [importer, usage] = importers.at(i);
    auto promise = importer.ImportBufferCollection(koid, sysmem_allocator_, std::move(tokens[i]),
                                                   usage, std::nullopt);
    promises.push_back(std::move(promise));
  }
  auto join_promise =
      fpromise::join_promise_vector(std::move(promises))
          .then([koid](fpromise::result<std::vector<fpromise::result<>>>& results)
                    -> fpromise::result<void, std::vector<fpromise::result<>>> {
            TRACE_DURATION_BEGIN("gfx", "allocation::Allocator::RegisterBufferCollection[end]");
            TRACE_FLOW_END("gfx", "allocation::Allocator::RegisterBufferCollection", koid);
            for (auto& result : results.value()) {
              if (!result.is_ok()) {
                return fpromise::error(results.take_value());
              }
            }
            return fpromise::ok();
          })
          // This `and_then` handles the case where all importers succeeded, and performs final
          // registration of the collection by adding it to the internal map.
          .and_then([this, koid, buffer_collection_usages,
                     export_token = std::move(export_token.value()),
                     completer = completer.share()]() mutable {
            buffer_collections_[koid] = buffer_collection_usages;

            // Use a self-referencing async::WaitOnce to deregister buffer collections when all
            // BufferCollectionImportTokens are used, i.e. peers of eventpair are closed. Note that
            // the ownership of |export_token| is also passed, so that GetRelatedKoid() calls return
            // valid koid.
            auto wait =
                std::make_shared<async::WaitOnce>(export_token.get(), ZX_EVENTPAIR_PEER_CLOSED);
            const zx_status_t status = wait->Begin(
                executor_.dispatcher(),
                [koid, keepalive_wait = wait, keepalive_export_token = std::move(export_token),
                 weak_this = weak_factory_.GetWeakPtr()](
                    async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                    const zx_packet_signal_t* /*signal*/) mutable {
                  FX_DCHECK(status == ZX_OK || status == ZX_ERR_CANCELED);
                  if (weak_this) {
                    // Because Flatland::CreateImage() holds an import token, this
                    // is guaranteed to be called after all images are created, so
                    // it is safe to release buffer collection.
                    weak_this->ReleaseBufferCollection(koid);
                  }
                });
            FX_DCHECK(status == ZX_OK);
            completer(fit::ok());
          })
          // This `or_else` handles the case where any importer failed. Some importers may have
          // succeeded; these must be told to release the collection.
          .or_else([this, koid, importers = std::move(importers), completer = std::move(completer)](
                       const std::vector<fpromise::result<>>& results) {
            // If any importers failed then clean up the ones that didn't before returning.
            for (uint32_t i = 0; i < results.size(); i++) {
              if (results[i].is_ok()) {
                auto& [importer, usage] = importers[i];
                importer.ReleaseBufferCollection(koid, usage);
              }
            }
            FX_LOGS(ERROR)
                << "Failed to import the buffer collection to the BufferCollectionimporter.";
            IncrementFailedBufferCollections();
            completer(fit::error(RegisterBufferCollectionError::kBadOperation));
          })
          .then([](const fpromise::result<>& result) {
            TRACE_DURATION_END("gfx", "allocation::Allocator::RegisterBufferCollection[end]");
          });
  executor_.schedule_task(std::move(join_promise));
}

void Allocator::ReleaseBufferCollection(GlobalBufferCollectionId collection_id) {
  TRACE_DURATION("gfx", "allocation::Allocator::ReleaseBufferCollection");
  FX_DCHECK(executor_.dispatcher() == async_get_default_dispatcher());

  const auto usages = buffer_collections_.at(collection_id);
  buffer_collections_.erase(collection_id);
  IncrementReleasedBufferCollections();

  for (auto& [importer, usage] : GetImporters(usages)) {
    importer.ReleaseBufferCollection(collection_id, usage);
  }
}

void Allocator::IncrementRegisteredBufferCollections() {
  inspect_registered_buffer_collections_.Add(1);
  inspect_outstanding_buffer_collections_.Add(1);
}

void Allocator::IncrementReleasedBufferCollections() {
  inspect_released_buffer_collections_.Add(1);
  inspect_outstanding_buffer_collections_.Subtract(1);
}

void Allocator::IncrementFailedBufferCollections() {
  inspect_failed_buffer_collections_.Add(1);
  inspect_outstanding_buffer_collections_.Subtract(1);
}

}  // namespace allocation
