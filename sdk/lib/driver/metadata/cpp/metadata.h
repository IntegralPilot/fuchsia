// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_METADATA_CPP_METADATA_H_
#define LIB_DRIVER_METADATA_CPP_METADATA_H_

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_metadata {

// Connects to the fuchsia.driver.metadata/Metadata FIDL protocol found within the |svc_dir|
// service directory at FIDL service |service_name| and instance |instance_name|.
zx::result<fidl::ClientEnd<fuchsia_driver_metadata::Metadata>> ConnectToMetadataProtocol(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir, std::string_view service_name,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance);

// Retrieves metadata from the fuchsia.driver.metadata/Metadata FIDL protocol within the |svc_dir|
// service directory found at FIDL service |service_name| and instance |instance_name|.
//
// Make sure that the component manifest specifies that it uses the `FidlType::kSerializableName`
// FIDL service.
template <typename FidlType>
#if __cplusplus >= 202002l
  requires(fidl::IsFidlTypeV<FidlType> && !fidl::IsResourceV<FidlType>)
#endif
zx::result<FidlType> GetMetadataFromFidlService(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir, std::string_view service_name,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  fidl::WireSyncClient<fuchsia_driver_metadata::Metadata> client{};
  {
    zx::result result = ConnectToMetadataProtocol(svc_dir, service_name, instance_name);
    if (result.is_error()) {
      fdf::error("Failed to connect to metadata server: {}", result.status_string());
      return result.take_error();
    }
    client.Bind(std::move(result.value()));
  }

  fidl::WireResult<fuchsia_driver_metadata::Metadata::GetPersistedMetadata> persisted_metadata =
      client->GetPersistedMetadata();
  if (!persisted_metadata.ok()) {
    fdf::error("Failed to send GetPersistedMetadata request: {}",
               persisted_metadata.status_string());
    return zx::error(persisted_metadata.status());
  }
  if (persisted_metadata->is_error()) {
    fdf::error("Failed to get persisted metadata: {}",
               zx_status_get_string(persisted_metadata->error_value()));
    return zx::error(persisted_metadata->error_value());
  }

  fit::result metadata =
      fidl::Unpersist<FidlType>(persisted_metadata.value()->persisted_metadata.get());
  if (metadata.is_error()) {
    fdf::error("Failed to unpersist metadata: {}",
               zx_status_get_string(metadata.error_value().status()));
    return zx::error(metadata.error_value().status());
  }

  return zx::ok(metadata.value());
}

// The same as `fdf_metadata::GetMetadataFromFidlService()` except that the service name is assumed
// to be `FidlType::kSerializableName`. Make sure that `FidlType` is annotated with `@serializable`.
template <typename FidlType>
zx::result<FidlType> GetMetadata(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  return GetMetadataFromFidlService<FidlType>(svc_dir, FidlType::kSerializableName, instance_name);
}

// The same as `fdf_metadata::GetMetadataFromFidlService()` except that the service name is assumed
// to be `FidlType::kSerializableName`. Make sure that `FidlType` is annotated with `@serializable`.
template <typename FidlType>
zx::result<FidlType> GetMetadata(
    const fdf::Namespace& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  return GetMetadata<FidlType>(incoming.svc_dir(), instance_name);
}

// The same as `fdf_metadata::GetMetadataFromFidlService()` except that the service name is assumed
// to be `FidlType::kSerializableName`. Make sure that `FidlType` is annotated with `@serializable`.
template <typename FidlType>
zx::result<FidlType> GetMetadata(
    const std::shared_ptr<fdf::Namespace>& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  return GetMetadata<FidlType>(incoming->svc_dir(), instance_name);
}

// This function is the same as `fdf_metadata::GetMetadata<FidlType>()` except that it will return a
// `std::nullopt` if there is no metadata FIDL protocol within |device|'s service directory at
// |instance_name| or if the FIDL server does not have metadata to provide.
template <typename FidlType>
#if __cplusplus >= 202002l
  requires(fidl::IsFidlTypeV<FidlType> && !fidl::IsResourceV<FidlType>)
#endif
zx::result<std::optional<FidlType>> GetMetadataFromFidlServiceIfExists(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir, std::string_view service_name,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  fidl::WireSyncClient<fuchsia_driver_metadata::Metadata> client{};
  {
    zx::result result = ConnectToMetadataProtocol(svc_dir, service_name, instance_name);
    if (result.is_error()) {
      fdf::debug("Failed to connect to metadata server: {}", result);
      return zx::ok(std::nullopt);
    }
    client.Bind(std::move(result.value()));
  }

  fidl::WireResult<fuchsia_driver_metadata::Metadata::GetPersistedMetadata> persisted_metadata =
      client->GetPersistedMetadata();
  if (!persisted_metadata.ok()) {
    if (persisted_metadata.status() == ZX_ERR_PEER_CLOSED) {
      // We assume that the metadata does not exist because we assume that the FIDL server does not
      // exist because we received a peer closed status.
      fdf::debug("Failed to send GetPersistedMetadata request: {}",
                 persisted_metadata.status_string());
      return zx::ok(std::nullopt);
    }
    fdf::error("Failed to send GetPersistedMetadata request: {}",
               persisted_metadata.status_string());
    return zx::error(persisted_metadata.status());
  }
  if (persisted_metadata->is_error()) {
    if (persisted_metadata->error_value() == ZX_ERR_NOT_FOUND) {
      fdf::debug("Failed to get persisted metadata: {}",
                 zx_status_get_string(persisted_metadata->error_value()));
      return zx::ok(std::nullopt);
    }
    fdf::error("Failed to get persisted metadata: {}",
               zx_status_get_string(persisted_metadata->error_value()));
    return zx::error(persisted_metadata->error_value());
  }

  fit::result metadata =
      fidl::Unpersist<FidlType>(persisted_metadata.value()->persisted_metadata.get());
  if (metadata.is_error()) {
    fdf::debug("Failed to unpersist metadata: {}", metadata.error_value().FormatDescription());
    return zx::error(metadata.error_value().status());
  }

  return zx::ok(metadata.value());
}

// The same as `fdf_metadata::GetMetadataFromFidlServiceIfExists()` except that the service name is
// assumed to be `FidlType::kSerializableName`. Make sure that `FidlType` is annotated with
// `@serializable`.
template <typename FidlType>
zx::result<std::optional<FidlType>> GetMetadataIfExists(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  return GetMetadataFromFidlServiceIfExists<FidlType>(svc_dir, FidlType::kSerializableName,
                                                      instance_name);
}

// The same as `fdf_metadata::GetMetadataFromFidlServiceIfExists()` except that the service name is
// assumed to be `FidlType::kSerializableName`. Make sure that `FidlType` is annotated with
// `@serializable`.
template <typename FidlType>
zx::result<std::optional<FidlType>> GetMetadataIfExists(
    const fdf::Namespace& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  return GetMetadataIfExists<FidlType>(incoming.svc_dir(), instance_name);
}

// The same as `fdf_metadata::GetMetadataFromFidlServiceIfExists()` except that the service name is
// assumed to be `FidlType::kSerializableName`. Make sure that `FidlType` is annotated with
// `@serializable`.
template <typename FidlType>
zx::result<std::optional<FidlType>> GetMetadataIfExists(
    const std::shared_ptr<fdf::Namespace>& incoming,
    std::string_view instance_name = component::OutgoingDirectory::kDefaultServiceInstance) {
  return GetMetadataIfExists<FidlType>(incoming->svc_dir(), instance_name);
}

}  // namespace fdf_metadata

#endif

#endif  // LIB_DRIVER_METADATA_CPP_METADATA_H_
