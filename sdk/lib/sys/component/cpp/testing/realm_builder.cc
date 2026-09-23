// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/component/cpp/fidl.h>
#include <fuchsia/component/decl/cpp/fidl.h>
#include <fuchsia/component/runner/cpp/fidl.h>
#include <fuchsia/component/test/cpp/fidl.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fidl/cpp/interface_handle.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/sys/component/cpp/testing/internal/convert.h>
#include <lib/sys/component/cpp/testing/internal/errors.h>
#include <lib/sys/component/cpp/testing/internal/local_component_runner.h>
#include <lib/sys/component/cpp/testing/internal/realm.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/sys/component/cpp/testing/scoped_child.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/service_directory.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/errors.h>

#include <cstddef>
#include <memory>
#include <optional>
#include <sstream>
#include <utility>
#include <vector>

namespace component_testing {
namespace {
constexpr char kFrameworkIntermediaryChildName[] = "realm_builder_server";
constexpr char kChildPathSeparator[] = "/";

fidl::InterfaceHandle<fuchsia::io::Directory> CreatePkgDirHandle() {
  int fd;
  ZX_COMPONENT_ASSERT_STATUS_OK(
      "fdio_open3_fd",
      // It's okay to cast Rights to Flags as there is a direct mapping.
      fdio_open3_fd("/pkg", static_cast<uint64_t>(fuchsia::io::RX_STAR_DIR), &fd));
  zx_handle_t handle;
  ZX_COMPONENT_ASSERT_STATUS_OK("fdio_fd_transfer", fdio_fd_transfer(fd, &handle));
  auto channel = zx::channel(handle);
  return fidl::InterfaceHandle<fuchsia::io::Directory>(std::move(channel));
}

}  // namespace

// Implementation methods for Realm.

Realm& Realm::AddChild(const std::string& child_name, const std::string& url,
                       const ChildOptions& options) {
  fuchsia::component::test::Realm_AddChild_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddChild",
      realm_proxy_->AddChild(child_name, url, internal::ConvertToFidl(options), &result), result);
  return *this;
}

// TODO(https://fxbug.dev/296292544): Remove when build support for API level 16 is removed.
// The newer definition of LocalComponentKind is incompatible with LocalComponent*.
#if FUCHSIA_API_LEVEL_LESS_THAN(17)
Realm& Realm::AddLocalChild(const std::string& child_name, LocalComponent* local_impl,
                            const ChildOptions& options) {
  return AddLocalChildImpl(child_name, LocalComponentKind(local_impl), options);
}
#endif

Realm& Realm::AddLocalChild(const std::string& child_name, LocalComponentFactory local_impl,
                            const ChildOptions& options) {
  return AddLocalChildImpl(child_name, LocalComponentKind(std::move(local_impl)), options);
}

Realm& Realm::AddLocalChildImpl(const std::string& child_name, LocalComponentKind local_impl,
                                const ChildOptions& options) {
// TODO(https://fxbug.dev/296292544): Remove when build support for API level 16 is removed.
#if FUCHSIA_API_LEVEL_LESS_THAN(17)
// Ignore warnings caused by the use of the deprecated `LocalComponent` type as it is part of the
// implementation that supports the deprecated type.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  if (std::holds_alternative<LocalComponent*>(local_impl)) {
    ZX_SYS_ASSERT_NOT_NULL(std::get<LocalComponent*>(local_impl));
  }
#pragma clang diagnostic pop
#endif
  runner_builder_->Register(GetResolvedName(child_name), std::move(local_impl));
  fuchsia::component::test::Realm_AddLocalChild_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddLocalChild",
      realm_proxy_->AddLocalChild(child_name, internal::ConvertToFidl(options), &result), result);
  return *this;
}

Realm Realm::AddChildRealm(const std::string& child_name, const ChildOptions& options) {
  fuchsia::component::test::RealmSyncPtr sub_realm_proxy;
  std::vector<std::string> sub_realm_scope = scope_;
  sub_realm_scope.push_back(child_name);
  Realm sub_realm(std::move(sub_realm_proxy), runner_builder_, std::move(sub_realm_scope));

  fuchsia::component::test::Realm_AddChildRealm_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddChildRealm",
      realm_proxy_->AddChildRealm(child_name, internal::ConvertToFidl(options),
                                  sub_realm.realm_proxy_.NewRequest(), &result),
      result);
  return sub_realm;
}

Realm Realm::AddChildRealmFromDecl(const std::string& child_name,
                                   fuchsia::component::decl::Component& decl,
                                   const ChildOptions& options) {
  fuchsia::component::test::RealmSyncPtr sub_realm_proxy;
  std::vector<std::string> sub_realm_scope = scope_;
  sub_realm_scope.push_back(child_name);
  Realm sub_realm(std::move(sub_realm_proxy), runner_builder_, std::move(sub_realm_scope));

  fuchsia::component::test::Realm_AddChildRealmFromDecl_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddChildRealmFromDecl",
      realm_proxy_->AddChildRealmFromDecl(child_name, std::move(decl),
                                          internal::ConvertToFidl(options),
                                          sub_realm.realm_proxy_.NewRequest(), &result),
      result);
  return sub_realm;
}

Realm& Realm::AddRoute(Route route) {
  auto capabilities = internal::ConvertToFidlVec<Capability, fuchsia::component::test::Capability>(
      route.capabilities);
  fuchsia::component::decl::Ref source;
  std::string from_dictionary;
  std::tie(source, from_dictionary) =
      internal::ConvertRefToFidl(route.source, internal::RefContext::SOURCE);
  std::vector<Ref> route_targets;
  for (auto t : std::move(route.targets)) {
    route_targets.emplace_back(t);
  }
  std::vector<fuchsia::component::decl::Ref> target;
  for (auto& t : internal::ConvertRefToFidlVec(route_targets, internal::RefContext::TARGET)) {
    target.push_back(std::move(t.first));
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(29)
  fuchsia::component::test::Realm_AddRouteFromDictionary_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddRouteFromDictionary",
      realm_proxy_->AddRouteFromDictionary(std::move(capabilities), std::move(source),
                                           std::move(from_dictionary), std::move(target), &result),
      result);
#else
  fuchsia::component::test::Realm_AddRoute_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddRoute",
      realm_proxy_->AddRoute(std::move(capabilities), std::move(source), std::move(target),
                             &result),
      result);
#endif
  return *this;
}

Realm& Realm::RouteReadOnlyDirectory(const std::string& name, std::vector<Ref> to,
                                     DirectoryContents directory) {
  std::vector<fuchsia::component::decl::Ref> to_fidl;
  to_fidl.reserve(to.size());
  for (auto& ref : to) {
    to_fidl.push_back(internal::ConvertRefToFidl(ref, internal::RefContext::SOURCE).first);
  }
  auto directory_fidl = directory.TakeAsFidl();

  fuchsia::component::test::Realm_ReadOnlyDirectory_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/ReadOnlyDirectory",
      realm_proxy_->ReadOnlyDirectory(name, std::move(to_fidl), std::move(directory_fidl), &result),
      result);

  return *this;
}

Realm& Realm::InitMutableConfigFromPackage(const std::string& name) {
  fuchsia::component::test::Realm_InitMutableConfigFromPackage_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/InitMutableConfigFromPackage",
      realm_proxy_->InitMutableConfigFromPackage(name, &result), result);
  return *this;
}

Realm& Realm::InitMutableConfigToEmpty(const std::string& name) {
  fuchsia::component::test::Realm_InitMutableConfigToEmpty_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK("Realm/InitMutableConfigToEmpty",
                                           realm_proxy_->InitMutableConfigToEmpty(name, &result),
                                           result);
  return *this;
}

Realm& Realm::SetConfigValue(const std::string& name, const std::string& key, ConfigValue value) {
  fuchsia::component::test::Realm_SetConfigValue_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/SetConfigValue", realm_proxy_->SetConfigValue(name, key, value.TakeAsFidl(), &result),
      result);
  return *this;
}

Realm& Realm::AddConfiguration(std::vector<ConfigCapability> configurations) {
  for (ConfigCapability& c : configurations) {
    fuchsia::component::decl::Configuration config;
    config.set_name(c.name);
    config.set_value(std::move(*c.value.TakeAsFidl().mutable_value()));
    fuchsia::component::test::Realm_AddCapability_Result result;
    ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
        "Realm/AddCapability",
        realm_proxy_->AddCapability(
            fuchsia::component::decl::Capability::WithConfig(std::move(config)), &result),
        result);
  }

  return *this;
}

Realm& Realm::AddCapability(fuchsia::component::decl::Capability capability) {
  fuchsia::component::test::Realm_AddCapability_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddCapability", realm_proxy_->AddCapability(std::move(capability), &result), result);
  return *this;
}

Realm& Realm::AddCollection(fuchsia::component::decl::Collection collection) {
  fuchsia::component::test::Realm_AddCollection_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddCollection", realm_proxy_->AddCollection(std::move(collection), &result), result);
  return *this;
}
Realm& Realm::AddEnvironment(fuchsia::component::decl::Environment environment) {
  fuchsia::component::test::Realm_AddEnvironment_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/AddEnvironment", realm_proxy_->AddEnvironment(std::move(environment), &result),
      result);
  return *this;
}

void Realm::ReplaceComponentDecl(const std::string& child_name,
                                 fuchsia::component::decl::Component decl) {
  fuchsia::component::test::Realm_ReplaceComponentDecl_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/ReplaceComponentDecl",
      realm_proxy_->ReplaceComponentDecl(child_name, std::move(decl), &result), result);
}

void Realm::ReplaceRealmDecl(fuchsia::component::decl::Component decl) {
  fuchsia::component::test::Realm_ReplaceRealmDecl_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/ReplaceRealmDecl", realm_proxy_->ReplaceRealmDecl(std::move(decl), &result), result);
}

fuchsia::component::decl::Component Realm::GetComponentDecl(const std::string& child_name) {
  fuchsia::component::test::Realm_GetComponentDecl_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Realm/GetComponentDecl", realm_proxy_->GetComponentDecl(child_name, &result), result);

  return std::move(result.response().component_decl);
}

fuchsia::component::decl::Component Realm::GetRealmDecl() {
  fuchsia::component::test::Realm_GetRealmDecl_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK("Realm/GetRealmDecl",
                                           realm_proxy_->GetRealmDecl(&result), result);

  return std::move(result.response().component_decl);
}

Realm::Realm(fuchsia::component::test::RealmSyncPtr realm_proxy,
             std::shared_ptr<internal::LocalComponentRunner::Builder> runner_builder,
             std::vector<std::string> scope)
    : realm_proxy_(std::move(realm_proxy)),
      runner_builder_(std::move(runner_builder)),
      scope_(std::move(scope)) {}

std::string Realm::GetResolvedName(const std::string& child_name) {
  if (scope_.empty()) {
    return child_name;
  }

  std::stringstream path;
  for (const auto& s : scope_) {
    path << s << kChildPathSeparator;
  }
  return path.str() + child_name;
}

// Implementation methods for RealmBuilder.

RealmBuilder RealmBuilder::Create(std::shared_ptr<sys::ServiceDirectory> svc) {
  return CreateImpl(std::nullopt, std::move(svc));
}

RealmBuilder RealmBuilder::CreateFromRelativeUrl(std::string_view fragment_only_url,
                                                 std::shared_ptr<sys::ServiceDirectory> svc) {
  return CreateImpl(fragment_only_url, std::move(svc));
}

RealmBuilder RealmBuilder::CreateImpl(std::optional<std::string_view> fragment_only_url,
                                      std::shared_ptr<sys::ServiceDirectory> svc) {
  if (svc == nullptr) {
    svc = sys::ServiceDirectory::CreateFromNamespace();
  }

  fuchsia::component::test::RealmBuilderFactorySyncPtr factory_proxy;
  auto realm_proxy = internal::CreateRealmPtr(svc);
  auto child_ref = fuchsia::component::decl::ChildRef{.name = kFrameworkIntermediaryChildName};
  auto exposed_dir = internal::OpenExposedDir(realm_proxy.get(), child_ref);
  zx_status_t status = fdio_service_connect_at(exposed_dir.channel().get(),
                                               fuchsia::component::test::RealmBuilderFactory::Name_,
                                               factory_proxy.NewRequest().TakeChannel().release());
  ZX_COMPONENT_ASSERT_STATUS_OK("RealmBuilderFactory/Create", status);
  fuchsia::component::test::BuilderSyncPtr builder_proxy;
  fuchsia::component::test::RealmSyncPtr test_realm_proxy;
  if (fragment_only_url.has_value()) {
    ZX_ASSERT_MSG(!fragment_only_url.value().empty(), "fragment_only_url can't be empty");

    fuchsia::component::test::RealmBuilderFactory_CreateFromRelativeUrl_Result result;
    ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
        "RealmBuilderFactory/CreateFromRelativeUrl",
        factory_proxy->CreateFromRelativeUrl(CreatePkgDirHandle(), fragment_only_url.value().data(),
                                             test_realm_proxy.NewRequest(),
                                             builder_proxy.NewRequest(), &result),
        result);
  } else {
    fuchsia::component::test::RealmBuilderFactory_Create_Result result;
    ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
        "RealmBuilderFactory/Create",
        factory_proxy->Create(CreatePkgDirHandle(), test_realm_proxy.NewRequest(),
                              builder_proxy.NewRequest(), &result),
        result);
  }
  return RealmBuilder(svc, std::move(builder_proxy), std::move(test_realm_proxy));
}

RealmBuilder& RealmBuilder::AddChild(const std::string& child_name, const std::string& url,
                                     const ChildOptions& options) {
  ZX_ASSERT_MSG(!child_name.empty(), "child_name can't be empty");
  ZX_ASSERT_MSG(!url.empty(), "url can't be empty");

  root_.AddChild(child_name, url, options);
  return *this;
}

// TODO(https://fxbug.dev/296292544): Remove when build support for API level 16 is removed.
// The newer definition of LocalComponentKind, which is a parameter to AddLocalChildImpl(), is
// incompatible with LocalComponent*.
#if FUCHSIA_API_LEVEL_LESS_THAN(17)
RealmBuilder& RealmBuilder::AddLocalChild(const std::string& child_name, LocalComponent* local_impl,
                                          const ChildOptions& options) {
  ZX_ASSERT_MSG(!child_name.empty(), "child_name can't be empty");
  ZX_ASSERT_MSG(local_impl != nullptr, "local_impl can't be nullptr");
  root_.AddLocalChildImpl(child_name, local_impl, options);
  return *this;
}
#endif

RealmBuilder& RealmBuilder::AddLocalChild(const std::string& child_name,
                                          LocalComponentFactory local_impl,
                                          const ChildOptions& options) {
  ZX_ASSERT_MSG(!child_name.empty(), "child_name can't be empty");
  root_.AddLocalChildImpl(child_name, LocalComponentKind(std::move(local_impl)), options);
  return *this;
}

Realm RealmBuilder::AddChildRealm(const std::string& child_name, const ChildOptions& options) {
  ZX_ASSERT_MSG(!child_name.empty(), "child_name can't be empty");
  return root_.AddChildRealm(child_name, options);
}

Realm RealmBuilder::AddChildRealmFromDecl(const std::string& child_name,
                                          fuchsia::component::decl::Component& decl,
                                          const ChildOptions& options) {
  ZX_ASSERT_MSG(!child_name.empty(), "child_name can't be empty");
  return root_.AddChildRealmFromDecl(child_name, decl, options);
}

RealmBuilder& RealmBuilder::AddRoute(Route route) {
  ZX_ASSERT_MSG(!route.capabilities.empty(), "route.capabilities can't be empty");
  ZX_ASSERT_MSG(!route.targets.empty(), "route.targets can't be empty");

  root_.AddRoute(std::move(route));
  return *this;
}

RealmBuilder& RealmBuilder::RouteReadOnlyDirectory(const std::string& name, std::vector<Ref> to,
                                                   DirectoryContents directory) {
  root_.RouteReadOnlyDirectory(name, std::move(to), std::move(directory));
  return *this;
}

RealmBuilder& RealmBuilder::InitMutableConfigFromPackage(const std::string& name) {
  root_.InitMutableConfigFromPackage(name);
  return *this;
}

RealmBuilder& RealmBuilder::InitMutableConfigToEmpty(const std::string& name) {
  root_.InitMutableConfigToEmpty(name);
  return *this;
}

RealmBuilder& RealmBuilder::AddConfiguration(std::vector<ConfigCapability> configurations) {
  root_.AddConfiguration(std::move(configurations));
  return *this;
}

RealmBuilder& RealmBuilder::AddCapability(fuchsia::component::decl::Capability capability) {
  root_.AddCapability(std::move(capability));
  return *this;
}

RealmBuilder& RealmBuilder::SetConfigValue(const std::string& name, const std::string& key,
                                           ConfigValue value) {
  root_.SetConfigValue(name, key, std::move(value));
  return *this;
}

void RealmBuilder::ReplaceComponentDecl(const std::string& child_name,
                                        fuchsia::component::decl::Component decl) {
  root_.ReplaceComponentDecl(child_name, std::move(decl));
}

void RealmBuilder::ReplaceRealmDecl(fuchsia::component::decl::Component decl) {
  root_.ReplaceRealmDecl(std::move(decl));
}

fuchsia::component::decl::Component RealmBuilder::GetComponentDecl(const std::string& child_name) {
  return root_.GetComponentDecl(child_name);
}

fuchsia::component::decl::Component RealmBuilder::GetRealmDecl() { return root_.GetRealmDecl(); }

RealmBuilder& RealmBuilder::SetRealmCollection(const std::string& collection) {
  realm_collection_ = collection;
  return *this;
}

RealmBuilder& RealmBuilder::SetRealmName(const std::string& name) {
  realm_name_ = name;
  return *this;
}

RealmRoot RealmBuilder::Build(async_dispatcher_t* dispatcher) {
  ZX_ASSERT_MSG(!realm_commited_, "Builder::Build() called after Realm already created");
  if (dispatcher == nullptr) {
    dispatcher = async_get_default_dispatcher();
  }
  ZX_ASSERT_MSG(dispatcher != nullptr, "Builder::Build() called without configured dispatcher");
  auto local_component_runner = runner_builder_->Build(dispatcher);
  fuchsia::component::test::Builder_Build_Result result;
  ZX_COMPONENT_ASSERT_STATUS_AND_RESULT_OK(
      "Builder/Build", builder_proxy_->Build(local_component_runner->NewBinding(), &result),
      result);
  realm_commited_ = true;

  auto scoped_child =
      realm_name_.has_value()
          ? ScopedChild::New(realm_collection_, realm_name_.value(),
                             result.response().root_component_url, svc_)
          : ScopedChild::New(realm_collection_, result.response().root_component_url, svc_);

  // Connect to fuchsia.component.Binder to automatically start Realm.
  if (start_on_build_) {
    scoped_child.ConnectSync<fuchsia::component::Binder>();
  }

  return RealmRoot(std::move(local_component_runner), std::move(scoped_child), dispatcher);
}

Realm& RealmBuilder::root() { return root_; }

RealmBuilder::RealmBuilder(std::shared_ptr<sys::ServiceDirectory> svc,
                           fuchsia::component::test::BuilderSyncPtr builder_proxy,
                           fuchsia::component::test::RealmSyncPtr test_realm_proxy)
    : svc_(std::move(svc)),
      builder_proxy_(std::move(builder_proxy)),
      runner_builder_(std::make_shared<internal::LocalComponentRunner::Builder>()),
      root_(Realm(std::move(test_realm_proxy), runner_builder_)) {}

// Implementation methods for RealmRoot.

RealmRoot::RealmRoot(std::unique_ptr<internal::LocalComponentRunner> local_component_runner,
                     ScopedChild root, async_dispatcher_t* dispatcher)
    : local_component_runner_(std::move(local_component_runner)),
      root_(std::move(root)),
      dispatcher_(dispatcher) {}

RealmRoot::~RealmRoot() = default;

void RealmRoot::Teardown(ScopedChild::TeardownCallback on_teardown_complete) {
  root_.Teardown(dispatcher_, std::move(on_teardown_complete));
}

ScopedChild& RealmRoot::component() { return root_; }

const ScopedChild& RealmRoot::component() const { return root_; }

}  // namespace component_testing
