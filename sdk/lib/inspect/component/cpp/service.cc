// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.inspect/cpp/markers.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fidl/cpp/wire/wire_types.h>
#include <lib/inspect/component/cpp/service.h>
#include <zircon/types.h>

#include <variant>
#include <vector>

namespace inspect {
void TreeNameIterator::StartSelfManagedServer(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_inspect::TreeNameIterator>&& request,
    std::vector<std::string> names) {
  ZX_ASSERT(dispatcher);

  auto impl = std::unique_ptr<TreeNameIterator>(new TreeNameIterator(std::move(names)));
  auto* ptr = impl.get();
  auto binding_ref = fidl::BindServer(dispatcher, std::move(request), std::move(impl), nullptr);
  ptr->binding_.emplace(std::move(binding_ref));
}

// Get the next batch of names. Names are sent in batches of `kMaxTreeNamesListSize`,
// which is defined with the rest of the FIDL protocol.
void TreeNameIterator::GetNext(GetNextCompleter::Sync& completer) {
  ZX_ASSERT(binding_.has_value());

  std::vector<fidl::StringView> converted_names;
  converted_names.reserve(names_.size());
  auto bytes_used = sizeof(fidl_message_header_t) + sizeof(fidl_vector_t);
  for (; current_index_ < names_.size(); current_index_++) {
    bytes_used += sizeof(fidl_string_t);
    bytes_used += FIDL_ALIGN(names_.at(current_index_).length());
    if (bytes_used > ZX_CHANNEL_MAX_MSG_BYTES) {
      break;
    }

    converted_names.emplace_back(fidl::StringView::FromExternal(names_.at(current_index_)));
  }

  completer.Reply(fidl::VectorView<fidl::StringView>::FromExternal(converted_names));
}

void TreeServer::StartSelfManagedServer(std::variant<Inspector, zx::vmo> data,
                                        TreeHandlerSettings settings,
                                        async_dispatcher_t* dispatcher,
                                        fidl::ServerEnd<fuchsia_inspect::Tree>&& request) {
  ZX_ASSERT(dispatcher);

  auto impl =
      std::unique_ptr<TreeServer>(new TreeServer(std::move(data), std::move(settings), dispatcher));

  auto* impl_ptr = impl.get();
  auto binding_ref = fidl::BindServer(dispatcher, std::move(request), std::move(impl), nullptr);
  impl_ptr->binding_.emplace(std::move(binding_ref));
}

void TreeServer::GetContent(GetContentCompleter::Sync& completer) {
  ZX_ASSERT(binding_.has_value());

  fidl::Arena arena;
  auto content_builder = fuchsia_inspect::wire::TreeContent::Builder(arena);
  fuchsia_mem::wire::Buffer buffer;
  const auto& primary_behavior = settings_.snapshot_behavior.PrimaryBehavior();
  const auto& failure_behavior = settings_.snapshot_behavior.FailureBehavior();
  using behavior_types = TreeServerSendPreference::Type;

  std::visit(
      [&](auto&& data) {
        // T is one of the variant values of data_; at the time of writing that is Inspector or
        // zx::vmo. The branching below first determines if the data is a VMO or an Inspector,
        // and then determines the duplication policy given the data type.
        using T = std::decay_t<decltype(data)>;
        if constexpr (std::is_same_v<T, Inspector>) {
          if (primary_behavior == behavior_types::Frozen) {
            auto maybe_vmo_frozen = data.FrozenVmoCopy();
            if (maybe_vmo_frozen.has_value()) {
              buffer.vmo = std::move(maybe_vmo_frozen.value());
            } else if (failure_behavior.has_value() && *failure_behavior == behavior_types::Live) {
              buffer.vmo = data.DuplicateVmo();
            } else {
              auto maybe_vmo_copied = data.CopyVmo();
              if (maybe_vmo_copied.has_value()) {
                buffer.vmo = std::move(maybe_vmo_copied.value());
              } else {
                buffer.vmo = data.DuplicateVmo();
              }
            }
          } else if (primary_behavior == behavior_types::Live) {
            buffer.vmo = data.DuplicateVmo();
          } else {
            auto maybe_vmo_copied = data.CopyVmo();
            if (maybe_vmo_copied.has_value()) {
              buffer.vmo = std::move(maybe_vmo_copied.value());
            } else {
              buffer.vmo = data.DuplicateVmo();
            }
          }
        } else if constexpr (std::is_same_v<T, zx::vmo>) {
          data.duplicate(ZX_RIGHTS_BASIC | ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY,
                         &buffer.vmo);
        }
      },
      data_);

  content_builder.buffer(std::move(buffer));
  completer.Reply(content_builder.Build());
}

void TreeServer::ListChildNames(ListChildNamesRequestView request,
                                ListChildNamesCompleter::Sync& completer) {
  ZX_ASSERT(binding_.has_value());

  std::visit(
      [&](auto&& data) {
        using T = std::decay_t<decltype(data)>;
        if constexpr (std::is_same_v<T, Inspector>) {
          TreeNameIterator::StartSelfManagedServer(
              executor_.dispatcher(), std::move(request->tree_iterator), data.GetChildNames());
        } else if constexpr (std::is_same_v<T, zx::vmo>) {
          TreeNameIterator::StartSelfManagedServer(executor_.dispatcher(),
                                                   std::move(request->tree_iterator), {});
        }
      },
      data_);
}

void TreeServer::OpenChild(OpenChildRequestView request, OpenChildCompleter::Sync& completer) {
  ZX_ASSERT(binding_.has_value());

  std::visit(
      [&](auto&& data) {
        using T = std::decay_t<decltype(data)>;
        if constexpr (std::is_same_v<T, Inspector>) {
          executor_.schedule_task(data.OpenChild(std::string(request->child_name.get()))
                                      .and_then([request = std::move(request->tree),
                                                 this](Inspector& inspector) mutable {
                                        TreeServer::StartSelfManagedServer(inspector, settings_,
                                                                           executor_.dispatcher(),
                                                                           std::move(request));
                                      }));
        }
      },
      data_);
}

}  // namespace inspect
