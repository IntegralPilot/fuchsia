// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/component/cpp/fidl.h>
#include <fuchsia/component/decl/cpp/fidl.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fit/defer.h>
#include <lib/sys/component/cpp/testing/internal/errors.h>
#include <lib/sys/component/cpp/testing/internal/realm.h>
#include <lib/sys/component/cpp/testing/scoped_child.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/service_directory.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <random>

namespace component_testing {

namespace {

std::size_t random_unsigned() {
  std::random_device random_device;
  std::mt19937 generator(random_device());
  std::uniform_int_distribution<std::size_t> distribution;
  return distribution(generator);
}
}  // namespace

ScopedChild ScopedChild::New(fuchsia::component::RealmSyncPtr realm_proxy, std::string collection,
                             std::string url) {
  std::string name = "auto-" + std::to_string(random_unsigned());
  return New(std::move(realm_proxy), std::move(collection), std::move(name), std::move(url));
}

ScopedChild ScopedChild::New(fuchsia::component::RealmSyncPtr realm_proxy, std::string collection,
                             std::string name, std::string url) {
  fuchsia::component::ControllerSyncPtr controller_proxy;
  internal::CreateChild(realm_proxy.get(), controller_proxy.NewRequest(), collection, name,
                        std::move(url));
  fuchsia::component::decl::ChildRef child_ref{.name = std::move(name),
                                               .collection = std::move(collection)};
  fuchsia::io::DirectorySyncPtr exposed_dir;
  exposed_dir.Bind(internal::OpenExposedDir(realm_proxy.get(), child_ref));
  return ScopedChild(sys::ServiceDirectory::CreateFromNamespace(), std::move(child_ref),
                     std::move(exposed_dir), std::move(controller_proxy));
}

ScopedChild ScopedChild::New(std::string collection, std::string name, std::string url,
                             std::shared_ptr<sys::ServiceDirectory> svc) {
  fuchsia::component::RealmSyncPtr realm_proxy;
  svc->Connect(realm_proxy.NewRequest());
  fuchsia::component::ControllerSyncPtr controller_proxy;
  internal::CreateChild(realm_proxy.get(), controller_proxy.NewRequest(), collection, name,
                        std::move(url));
  fuchsia::component::decl::ChildRef child_ref{.name = std::move(name),
                                               .collection = std::move(collection)};
  fuchsia::io::DirectorySyncPtr exposed_dir;
  exposed_dir.Bind(internal::OpenExposedDir(realm_proxy.get(), child_ref));
  return ScopedChild(std::move(svc), std::move(child_ref), std::move(exposed_dir),
                     std::move(controller_proxy));
}

ScopedChild ScopedChild::New(std::string collection, std::string url,
                             std::shared_ptr<sys::ServiceDirectory> svc) {
  std::string name = "auto-" + std::to_string(random_unsigned());
  return New(std::move(collection), std::move(name), std::move(url), std::move(svc));
}

ScopedChild::ScopedChild(std::shared_ptr<sys::ServiceDirectory> svc,
                         fuchsia::component::decl::ChildRef child_ref,
                         fuchsia::io::DirectorySyncPtr exposed_dir,
                         fuchsia::component::ControllerSyncPtr controller_proxy)
    : svc_(std::move(svc)),
      child_ref_(std::move(child_ref)),
      exposed_dir_(std::move(exposed_dir)),
      controller_proxy_(std::move(controller_proxy)) {}

ScopedChild::~ScopedChild() {
  if (svc_ == nullptr) {
    return;
  }

  fuchsia::component::RealmSyncPtr sync_realm_proxy;
  ZX_COMPONENT_ASSERT_STATUS_OK("sys::ServiceDirectory/Connect",
                                svc_->Connect(sync_realm_proxy.NewRequest()));
  internal::DestroyChild(sync_realm_proxy.get(), child_ref_);
}

void ScopedChild::Teardown(async_dispatcher_t* dispatcher, TeardownCallback callback) {
  ZX_ASSERT(callback != nullptr);

  if (svc_ == nullptr) {
    callback(fit::ok());
    return;
  }

  fuchsia::component::RealmPtr async_realm_proxy;
  async_realm_proxy.set_error_handler(
      [](zx_status_t status) { ZX_PANIC("%s", zx_status_get_string(status)); });
  ZX_COMPONENT_ASSERT_STATUS_OK("sys::ServiceDirectory/Connect",
                                svc_->Connect(async_realm_proxy.NewRequest(dispatcher)));

  internal::DestroyChild(
      async_realm_proxy.get(), child_ref_,
      [callback = std::move(callback),
       // Stash a panic-on-drop callback into the closure; this asserts that callers do not shut
       // down the dispatcher before the async call completes.
       forbid_drop = fit::deferred_callback(
           []() { ZX_PANIC("async callback dropped without being called"); }),
       // We have to move the proxy into the callback so that the channel
       // connection doesn't prematurely close.
       _proxy = std::move(async_realm_proxy)](
          fuchsia::component::Realm_DestroyChild_Result result) mutable {
        forbid_drop.cancel();
        switch (result.Which()) {
          case fuchsia::component::Realm_DestroyChild_Result::kResponse:
            callback(fit::ok());
            break;
          case fuchsia::component::Realm_DestroyChild_Result::kErr:
            callback(fit::error(result.err()));
            break;
          case fuchsia::component::Realm_DestroyChild_Result::Invalid:
            ZX_PANIC("Realm/DestroyChild returned invalid response");
        }
      });

  // Prevent the destructor from calling DestroyChild again.
  svc_ = nullptr;
}

zx_status_t ScopedChild::Connect(const std::string& interface_name, zx::channel request) const {
  return fdio_service_connect_at(exposed_dir_.unowned_channel()->get(), interface_name.c_str(),
                                 request.release());
}

std::string ScopedChild::GetChildName() const { return child_ref_.name; }

const fuchsia::io::DirectorySyncPtr& ScopedChild::exposed() const { return exposed_dir_; }

ExecutionController ScopedChild::Start(fuchsia::component::StartChildArgs start_args) const {
  fuchsia::component::Controller_Start_Result result;
  fuchsia::component::ExecutionControllerPtr execution_controller_proxy;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Controller/Start",
      controller_proxy_->Start(std::move(start_args), execution_controller_proxy.NewRequest(),
                               &result),
      result);
  return ExecutionController(std::move(execution_controller_proxy));
}

}  // namespace component_testing
