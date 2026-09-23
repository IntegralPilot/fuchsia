// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYS_CPP_TESTING_SERVICE_DIRECTORY_PROVIDER_H_
#define LIB_SYS_CPP_TESTING_SERVICE_DIRECTORY_PROVIDER_H_

#include <lib/fidl/cpp/interface_request.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>

namespace vfs {
class Service;
class PseudoDir;
}  // namespace vfs

namespace sys {
namespace testing {

// This provides a fake |ServiceDirectory| for unit testing.
// Provides access to services that have been added to this object.
// The object of this class should be kept alive for fake |ServiceDirectory| to
// work.
//
// This class is thread-hostile.
//
//  # Simple usage
//
// Instances of this class should be owned and managed on the same thread.
//
// # Advanced usage
//
// You can use a background thread to service this class provided:
// async_dispatcher_t for the background thread is stopped or suspended
// prior to destroying the class object.
class ServiceDirectoryProvider {
 public:
  explicit ServiceDirectoryProvider(async_dispatcher_t* dispatcher = nullptr);

  ~ServiceDirectoryProvider();

  // Injects a service which can be accessed by calling Connect on
  // |sys::ServiceDirectory| by code under test.
  //
  // Adds a supported service with the given |service_name|, using the given
  // |interface_request_handler|. |interface_request_handler| should
  // remain valid for the lifetime of this object.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: This already contains an entry for
  // this service.
  //
  // # Example
  //
  // ```
  // fidl::BindingSet<fuchsia::foo::Controller> bindings;
  // svc->AddService(bindings.GetHandler(this));
  // ```
  template <typename Interface>
  zx_status_t AddService(fidl::InterfaceRequestHandler<Interface> handler,
                         std::string name = Interface::Name_) const {
    return AddService(
        [handler = std::move(handler)](zx::channel channel, async_dispatcher_t* dispatcher) {
          handler(fidl::InterfaceRequest<Interface>(std::move(channel)));
        },
        std::move(name));
  }

  // Injects a service which can be accessed by calling Connect on
  // |sys::ServiceDirectory| by code under test.
  //
  // Adds a supported service with the given |service_name|, using the given
  // |service|. |service| closure should
  // remain valid for the lifetime of this object.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: This already contains an entry for
  // this service.
  zx_status_t AddService(std::unique_ptr<vfs::Service> service, std::string name) const;

  std::shared_ptr<ServiceDirectory>& service_directory() { return service_directory_; }

 private:
  using Connector = fit::function<void(zx::channel channel, async_dispatcher_t* dispatcher)>;

  zx_status_t AddService(Connector connector, std::string service_name) const;

  std::shared_ptr<ServiceDirectory> service_directory_;
  std::unique_ptr<vfs::PseudoDir> svc_dir_;
};

}  // namespace testing
}  // namespace sys

#endif  // LIB_SYS_CPP_TESTING_SERVICE_DIRECTORY_PROVIDER_H_
