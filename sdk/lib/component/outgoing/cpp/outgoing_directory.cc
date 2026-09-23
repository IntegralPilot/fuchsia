// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/outgoing/cpp/handlers.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/svc/dir.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/types.h>

#include <memory>
#include <sstream>
#include <string>

namespace {

// Path delimiter used by svc library.
constexpr const char kSvcPathDelimiter[] = "/";

constexpr const char kOutgoingDirectoryThreadSafetyDescription[] =
    "|component::OutgoingDirectory| is thread-unsafe.";

}  // namespace

namespace component {

OutgoingDirectory::OutgoingDirectory(async_dispatcher_t* dispatcher) {
  ZX_ASSERT_MSG(dispatcher != nullptr, "OutgoingDirectory::Create received nullptr |dispatcher|.");

  svc_dir_t* root = nullptr;
  // It's safe to ignore return value here since the function always returns ZX_OK.
  (void)svc_directory_create(&root);

  inner_ = std::make_unique<Inner>(dispatcher, root);
}

OutgoingDirectory::Inner::Inner(async_dispatcher_t* dispatcher, svc_dir_t* root)
    : dispatcher_(dispatcher),
      checker_(dispatcher, kOutgoingDirectoryThreadSafetyDescription),
      root_(root) {
  synchronization_check_dispatcher_task_.emplace(
      [this](async_dispatcher_t* dispatcher, async::Task* task, zx_status_t status) {
        if (status == ZX_ERR_CANCELED) {
          // This can happen if the dispatcher was shut down. Since the dispatcher is being
          // joined, we can avoid checking threads and still know that there's no
          // concurrent access.
          return;
        }
        ZX_ASSERT_MSG(status == ZX_OK,
                      "The synchronization checker task encountered an "
                      "unexpected status: %s",
                      zx_status_get_string(status));
        std::lock_guard guard(checker_);
      });
  synchronization_check_dispatcher_task_.value().Post(dispatcher);
}

OutgoingDirectory::OutgoingDirectory(OutgoingDirectory&& other) noexcept = default;

OutgoingDirectory& OutgoingDirectory::operator=(OutgoingDirectory&& other) noexcept = default;

OutgoingDirectory::~OutgoingDirectory() = default;

OutgoingDirectory::Inner::~Inner() {
  std::lock_guard guard(checker_);

  // Close all active connections managed by |OutgoingDirectory|.
  for (auto& [k, callbacks] : unbind_protocol_callbacks_) {
    for (auto& cb : callbacks) {
      cb();
    }
  }

  if (root_) {
    svc_directory_destroy(root_);
  }
}

zx::result<> OutgoingDirectory::Serve(fidl::ServerEnd<fuchsia_io::Directory> directory_server_end) {
  if (!directory_server_end.is_valid()) {
    return zx::error(ZX_ERR_BAD_HANDLE);
  }
  std::lock_guard guard(inner().checker_);

  return zx::make_result(svc_directory_serve(inner().root_, inner().dispatcher_,
                                             directory_server_end.TakeHandle().release()));
}

zx::result<> OutgoingDirectory::ServeFromStartupInfo() {
  fidl::ServerEnd<fuchsia_io::Directory> directory_request(
      zx::channel(zx_take_startup_handle(PA_DIRECTORY_REQUEST)));
  return Serve(std::move(directory_request));
}

zx::result<> OutgoingDirectory::AddUnmanagedProtocol(AnyHandler handler, std::string_view name) {
  return AddUnmanagedProtocolAt(std::move(handler), kServiceDirectory, name);
}

zx::result<> OutgoingDirectory::AddUnmanagedProtocolAt(AnyHandler handler, std::string_view path,
                                                       std::string_view name) {
  std::lock_guard guard(inner().checker_);

  // More thorough path validation is done in |svc_add_service|.
  if (path.empty() || name.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::string directory_entry(path);
  std::string protocol_entry(name);
  if (inner().registered_handlers_.count(directory_entry) != 0 &&
      inner().registered_handlers_[directory_entry].count(protocol_entry) != 0) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  // |svc_dir_add_service_by_path| takes in a void* |context| that is passed to
  // the |handler| callback passed as the last argument to the function call.
  // The context will first be stored in the heap for this path, then a pointer
  // to it will be passed to this function. The callback, in this case |OnConnect|,
  // will then cast the void* type to OnConnectContext*.
  auto context =
      std::make_unique<OnConnectContext>(OnConnectContext{.handler = std::move(handler)});
  if (zx_status_t status =
          svc_directory_add_service(inner().root_, directory_entry.c_str(), directory_entry.size(),
                                    name.data(), name.size(), context.get(), OnConnect);
      status != ZX_OK) {
    return zx::error(status);
  }

  auto& directory_handlers = inner().registered_handlers_[directory_entry];
  directory_handlers[protocol_entry] = std::move(context);

  return zx::ok();
}

zx::result<> OutgoingDirectory::RemoveUnmanagedProtocolAt(std::string_view path,
                                                          std::string_view name) {
  std::lock_guard guard(inner().checker_);

  // More thorough path validation is done in |svc_add_service|.
  if (path.empty() || name.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::string directory_entry(path);
  std::string protocol_entry(name);
  if (inner().registered_handlers_.count(directory_entry) == 0 ||
      inner().registered_handlers_[directory_entry].count(protocol_entry) == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  if (zx_status_t status = svc_directory_remove_entry(
          inner().root_, directory_entry.c_str(), directory_entry.size(), name.data(), name.size());
      status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok();
}

zx::result<> OutgoingDirectory::AddDirectory(fidl::ClientEnd<fuchsia_io::Directory> remote_dir,
                                             std::string_view directory_name) {
  return AddDirectoryAt(std::move(remote_dir), /*path=*/"", directory_name);
}

zx::result<> OutgoingDirectory::AddDirectoryAt(fidl::ClientEnd<fuchsia_io::Directory> remote_dir,
                                               std::string_view path,
                                               std::string_view directory_name) {
  std::lock_guard guard(inner().checker_);

  if (!remote_dir.is_valid()) {
    return zx::error(ZX_ERR_BAD_HANDLE);
  }
  if (directory_name.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::make_result(svc_directory_add_directory(inner().root_, path.data(), path.size(),
                                                     directory_name.data(), directory_name.size(),
                                                     remote_dir.TakeChannel().release()));
}

zx::result<> OutgoingDirectory::AddService(ServiceInstanceHandler handler, std::string_view service,
                                           std::string_view instance) {
  return AddServiceAt(std::move(handler), kServiceDirectory, service, instance);
}

zx::result<> OutgoingDirectory::AddServiceAt(ServiceInstanceHandler handler, std::string_view path,
                                             std::string_view service, std::string_view instance) {
  if (service.empty() || instance.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto handlers = handler.TakeMemberHandlers();
  if (handlers.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::string fullpath = MakePath({path, service, instance});
  for (auto& [member_name, member_handler] : handlers) {
    zx::result<> result = AddUnmanagedProtocolAt(std::move(member_handler), fullpath, member_name);
    if (result.is_error()) {
      // If we encounter an error with any of the instance members, scrub entire
      // directory entry.
      for (auto& [member_name_2, member_handler_2] : handlers) {
        // It's possible that we'll get an error from this call if we hit an
        // error before attempting to add everything in our list. The error is
        // harmless, so we ignore the return value.
        zx::result<> result_2 = RemoveUnmanagedProtocolAt(fullpath, member_name_2);
        (void)result_2;
      }
      inner().registered_handlers_.erase(fullpath);
      return result;
    }
  }

  return zx::ok();
}

zx::result<> OutgoingDirectory::RemoveProtocol(std::string_view name) {
  return RemoveProtocolAt(kServiceDirectory, name);
}

zx::result<> OutgoingDirectory::RemoveProtocolAt(std::string_view directory,
                                                 std::string_view name) {
  std::lock_guard guard(inner().checker_);

  std::string key(directory);

  if (inner().registered_handlers_.count(key) == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto& svc_root_handlers = inner().registered_handlers_[key];
  std::string entry_key = std::string(name);
  if (svc_root_handlers.count(entry_key) == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Remove svc_dir_t entry first so that no new connections are attempted on
  // handler after we remove the pointer to it in |svc_root_handlers|.
  if (zx_status_t status = svc_directory_remove_entry(inner().root_, key.c_str(), key.size(),
                                                      name.data(), name.size());
      status != ZX_OK) {
    return zx::error(status);
  }

  // If teardown is managed, e.g. through |AddProtocol| overload,
  // then close all active connections.
  UnbindAllConnections(name);

  // Now that all active connections have been closed, and no new connections
  // are being accepted under |name|, it's safe to remove the handlers.
  svc_root_handlers.erase(entry_key);

  return zx::ok();
}

zx::result<> OutgoingDirectory::RemoveService(std::string_view service, std::string_view instance) {
  return RemoveServiceAt(kServiceDirectory, service, instance);
}

zx::result<> OutgoingDirectory::RemoveServiceAt(std::string_view path, std::string_view service,
                                                std::string_view instance) {
  std::lock_guard guard(inner().checker_);

  std::string fullpath = MakePath({path, service, instance});
  if (inner().registered_handlers_.count(fullpath) == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Remove svc_dir_t entry first so that channels close _before_ we remove
  // pointer values out from underneath handlers.
  std::string service_path = MakePath({path, service});
  if (zx_status_t status =
          svc_directory_remove_entry(inner().root_, service_path.c_str(), service_path.size(),
                                     instance.data(), instance.size());
      status != ZX_OK) {
    return zx::error(status);
  }

  // Now it's safe to remove entry from map.
  inner().registered_handlers_.erase(fullpath);

  return zx::ok();
}

zx::result<> OutgoingDirectory::RemoveDirectory(std::string_view directory_name) {
  return RemoveDirectoryAt(/*path=*/"", directory_name);
}

zx::result<> OutgoingDirectory::RemoveDirectoryAt(std::string_view path,
                                                  std::string_view directory_name) {
  std::lock_guard guard(inner().checker_);

  if (directory_name.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::make_result(svc_directory_remove_entry(inner().root_, path.data(), path.size(),
                                                    directory_name.data(), directory_name.size()));
}

void OutgoingDirectory::OnConnect(void* raw_context, const char* service_name, zx_handle_t handle) {
  OnConnectContext* context = reinterpret_cast<OnConnectContext*>(raw_context);
  (context->handler)(zx::channel(handle));
}

void OutgoingDirectory::AppendUnbindConnectionCallback(UnbindCallbackMap* unbind_protocol_callbacks,
                                                       const std::string& name,
                                                       UnbindConnectionCallback callback) {
  (*unbind_protocol_callbacks)[name].emplace_back(std::move(callback));
}

void OutgoingDirectory::UnbindAllConnections(std::string_view name) {
  auto key = std::string(name);
  auto iterator = inner().unbind_protocol_callbacks_.find(key);
  if (iterator != inner().unbind_protocol_callbacks_.end()) {
    std::vector<UnbindConnectionCallback>& callbacks = iterator->second;
    for (auto& cb : callbacks) {
      cb();
    }
    inner().unbind_protocol_callbacks_.erase(iterator);
  }
}

std::string OutgoingDirectory::MakePath(std::vector<std::string_view> strings) {
  size_t output_length = 0;
  // Add up the sizes of the strings.
  for (const auto& i : strings) {
    output_length += i.size();
  }
  // Add the sizes of the separators.
  if (!strings.empty()) {
    output_length += (strings.size() - 1) * std::string(kSvcPathDelimiter).size();
  }

  std::string joined;
  joined.reserve(output_length);

  bool first = true;
  for (const auto& i : strings) {
    if (!first) {
      joined.append(kSvcPathDelimiter);
    } else {
      first = false;
    }
    joined.append(i.begin(), i.end());
  }

  return joined;
}

}  // namespace component
