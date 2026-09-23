// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPAT_CPP_METADATA_H_
#define LIB_DRIVER_COMPAT_CPP_METADATA_H_

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <lib/driver/async-helpers/cpp/async_task.h>
#include <lib/driver/incoming/cpp/namespace.h>

#include <memory>
#include <string_view>

namespace compat {

namespace internal {
template <typename ReturnType, typename Unpersist, typename NamespacePtr>
zx::result<ReturnType> GetMetadata(const NamespacePtr& incoming, uint32_t type,
                                   std::string_view instance, Unpersist unpersist) {
  zx::result compat = incoming->template Connect<fuchsia_driver_compat::Service::Device>(instance);
  if (compat.is_error()) {
    return compat.take_error();
  }

  fidl::WireResult metadata_result = fidl::WireCall(*compat)->GetMetadata();
  if (!metadata_result.ok()) {
    return zx::error(metadata_result.status());
  }
  if (metadata_result->is_error()) {
    return metadata_result->take_error();
  }

  auto& metadata_vector = metadata_result.value()->metadata;
  for (const auto& metadata : metadata_vector) {
    if (metadata.type != type) {
      continue;
    }
    size_t size;
    zx_status_t status = metadata.data.get_prop_content_size(&size);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    return unpersist(metadata.data, size);
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

template <typename ReturnType, typename Unpersist>
fdf::async_helpers::AsyncTask GetMetadataAsync(
    async_dispatcher_t* dispatcher, const std::shared_ptr<fdf::Namespace>& incoming, uint32_t type,
    std::string_view instance, Unpersist unpersist,
    fit::callback<void(zx::result<ReturnType>)> callback) {
  zx::result compat = incoming->Connect<fuchsia_driver_compat::Service::Device>(instance);
  if (compat.is_error()) {
    callback(compat.take_error());
    return fdf::async_helpers::AsyncTask(true);
  }

  fdf::async_helpers::AsyncTask task;
  fidl::WireClient compat_client(std::move(compat.value()), dispatcher);
  compat_client->GetMetadata().Then(
      [completer = task.CreateCompleter(), cb = std::move(callback),
       unpersist_func = std::move(unpersist),
       type](fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetMetadata>&
                 metadata_result) mutable {
        if (!metadata_result.ok()) {
          cb(zx::error(metadata_result.status()));
          return;
        }
        if (metadata_result->is_error()) {
          cb(metadata_result->take_error());
          return;
        }

        auto& metadata_vector = metadata_result.value()->metadata;
        for (const auto& metadata : metadata_vector) {
          if (metadata.type != type) {
            continue;
          }
          size_t size;
          zx_status_t status = metadata.data.get_prop_content_size(&size);
          if (status != ZX_OK) {
            cb(zx::error(status));
            return;
          }

          cb(unpersist_func(metadata.data, size));
          return;
        }

        cb(zx::error(ZX_ERR_NOT_FOUND));
      });
  task.SetItem(std::move(compat_client));
  return task;
}

}  // namespace internal

// Attempts to talk to a parent to acquire metadata with |type| and then decodes it into |FidlType|.
template <typename FidlType, typename NamespacePtr,
          typename = std::enable_if_t<fidl::IsFidlObject<FidlType>::value>>
zx::result<FidlType> GetMetadata(const NamespacePtr& incoming, uint32_t type,
                                 std::string_view instance = "default") {
  return internal::GetMetadata<FidlType>(
      incoming, type, instance, [](const zx::vmo& vmo, size_t size) -> zx::result<FidlType> {
        auto buffer = std::make_unique<uint8_t[]>(size);
        zx_status_t status = vmo.read(buffer.get(), 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }

        fit::result result = fidl::Unpersist<FidlType>(cpp20::span(buffer.get(), size));
        if (result.is_error()) {
          return zx::error(result.error_value().status());
        }
        return zx::ok(*result);
      });
}

// Attempts to talk to a parent to acquire metadata with |type| and then decodes it into |FidlType|.
template <typename FidlType, typename = std::enable_if_t<fidl::IsFidlObject<FidlType>::value>>
fdf::async_helpers::AsyncTask GetMetadataAsync(async_dispatcher_t* dispatcher,
                                               const std::shared_ptr<fdf::Namespace>& incoming,
                                               uint32_t type,
                                               fit::callback<void(zx::result<FidlType>)> callback,
                                               std::string_view instance = "default") {
  return internal::GetMetadataAsync<FidlType>(
      dispatcher, incoming, type, instance,
      [](const zx::vmo& vmo, size_t size) -> zx::result<FidlType> {
        auto buffer = std::make_unique<uint8_t[]>(size);
        zx_status_t status = vmo.read(buffer.get(), 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }

        fit::result result = fidl::Unpersist<FidlType>(cpp20::span(buffer.get(), size));
        if (result.is_error()) {
          return zx::error(result.error_value().status());
        }
        return zx::ok(*result);
      },
      std::move(callback));
}

// Attempts to talk to a parent to acquire metadata with |type| and then decodes it into |FidlType|.
// The result is only valid as long as |arena| remains alive.
template <typename FidlType, typename NamespacePtr>
#if __cplusplus >= 202002l
  requires fidl::IsFidlObject<FidlType>::value
#endif
zx::result<fidl::ObjectView<FidlType>> GetMetadata(const NamespacePtr& incoming,
                                                   fidl::AnyArena& arena, uint32_t type,
                                                   std::string_view instance = "default") {
  return internal::GetMetadata<fidl::ObjectView<FidlType>>(
      incoming, type, instance,
      [&arena](const zx::vmo& vmo, size_t size) -> zx::result<fidl::ObjectView<FidlType>> {
        auto buffer = arena.AllocateVector<uint8_t>(size);
        zx_status_t status = vmo.read(buffer, 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }

        fit::result result = fidl::InplaceUnpersist<FidlType>(cpp20::span(buffer, size));
        if (result.is_error()) {
          return zx::error(result.error_value().status());
        }
        return zx::ok(*result);
      });
}

// Attempts to talk to a parent to acquire metadata with |type| and then decodes it into |FidlType|.
// The result is only valid as long as |arena| remains alive.
template <typename FidlType>
#if __cplusplus >= 202002l
  requires fidl::IsFidlObject<FidlType>::value
#endif
fdf::async_helpers::AsyncTask GetMetadataAsync(
    async_dispatcher_t* dispatcher, const std::shared_ptr<fdf::Namespace>& incoming,
    fidl::AnyArena& arena, uint32_t type,
    fit::callback<void(zx::result<fidl::ObjectView<FidlType>>)> callback,
    std::string_view instance = "default") {
  return internal::GetMetadataAsync<fidl::ObjectView<FidlType>>(
      dispatcher, incoming, type, instance,
      [&arena](const zx::vmo& vmo, size_t size) -> zx::result<fidl::ObjectView<FidlType>> {
        auto buffer = arena.AllocateVector<uint8_t>(size);
        zx_status_t status = vmo.read(buffer, 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }

        fit::result result = fidl::InplaceUnpersist<FidlType>(cpp20::span(buffer, size));
        if (result.is_error()) {
          return zx::error(result.error_value().status());
        }
        return zx::ok(*result);
      },
      std::move(callback));
}

// Attempts to talk to a parent to acquire metadata with |type| and then casts it into |ReturnType|.
template <typename ReturnType, typename NamespacePtr,
          typename = std::enable_if_t<!fidl::IsFidlObject<ReturnType>::value>>
zx::result<std::unique_ptr<ReturnType>> GetMetadata(const NamespacePtr& incoming, uint32_t type,
                                                    std::string_view instance = "default") {
  return internal::GetMetadata<std::unique_ptr<ReturnType>>(
      incoming, type, instance,
      [](const zx::vmo& vmo, size_t size) -> zx::result<std::unique_ptr<ReturnType>> {
        if (size != sizeof(ReturnType)) {
          return zx::error(ZX_ERR_INTERNAL);
        }
        auto ret = std::make_unique<ReturnType>();
        zx_status_t status = vmo.read(ret.get(), 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }
        return zx::ok(std::move(ret));
      });
}

// Attempts to talk to a parent to acquire metadata with |type| and then casts it into |ReturnType|.
template <typename ReturnType, typename = std::enable_if_t<!fidl::IsFidlObject<ReturnType>::value>>
fdf::async_helpers::AsyncTask GetMetadataAsync(
    async_dispatcher_t* dispatcher, const std::shared_ptr<fdf::Namespace>& incoming, uint32_t type,
    fit::callback<void(zx::result<std::unique_ptr<ReturnType>>)> callback,
    std::string_view instance = "default") {
  return internal::GetMetadataAsync<std::unique_ptr<ReturnType>>(
      dispatcher, incoming, type, instance,
      [](const zx::vmo& vmo, size_t size) -> zx::result<std::unique_ptr<ReturnType>> {
        if (size != sizeof(ReturnType)) {
          return zx::error(ZX_ERR_INTERNAL);
        }
        auto ret = std::make_unique<ReturnType>();
        zx_status_t status = vmo.read(ret.get(), 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }
        return zx::ok(std::move(ret));
      },
      std::move(callback));
}

// Attempts to talk to a parent to acquire an array of metadata with |type| and
// stores it in a `std::vector<ReturnType>`.
template <typename ReturnType, typename NamespacePtr,
          typename = std::enable_if_t<!fidl::IsFidlObject<ReturnType>::value>,
          typename = std::enable_if_t<std::is_trivial_v<ReturnType>>>
zx::result<std::vector<ReturnType>> GetMetadataArray(const NamespacePtr& incoming, uint32_t type,
                                                     std::string_view instance = "default") {
  return internal::GetMetadata<std::vector<ReturnType>>(
      incoming, type, instance,
      [](const zx::vmo& vmo, size_t size) -> zx::result<std::vector<ReturnType>> {
        if (size % sizeof(ReturnType) != 0) {
          return zx::error(ZX_ERR_INTERNAL);
        }
        auto count = size / sizeof(ReturnType);
        auto ret = std::vector<ReturnType>();
        ret.resize(count);
        zx_status_t status = vmo.read(ret.data(), 0, size);
        if (status != ZX_OK) {
          return zx::error(status);
        }
        return zx::ok(std::move(ret));
      });
}

}  // namespace compat

#endif  // LIB_DRIVER_COMPAT_CPP_METADATA_H_
