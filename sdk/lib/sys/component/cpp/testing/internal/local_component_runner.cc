// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/component/runner/cpp/fidl.h>
#include <fuchsia/component/test/cpp/fidl.h>
#include <fuchsia/data/cpp/fidl.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/sys/component/cpp/testing/internal/errors.h>
#include <lib/sys/component/cpp/testing/internal/local_component_runner.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/status.h>

#include <cstddef>
#include <iterator>
#include <memory>
#include <optional>
#include <variant>

namespace component_testing {
namespace internal {

namespace {

std::string ExtractLocalComponentName(const fuchsia::data::Dictionary& program) {
  ZX_ASSERT_MSG(program.has_entries(), "Received empty program from Component Manager");
  for (const auto& entry : program.entries()) {
    if (entry.key == fuchsia::component::test::LOCAL_COMPONENT_NAME_KEY) {
      ZX_ASSERT_MSG(entry.value->is_str(), "Received local component key of wrong type");
      return entry.value->str();
    }
  }

  ZX_PANIC("Received program without local component key");
}

}  // namespace

LocalComponentInstance::LocalComponentInstance(
    fidl::InterfaceRequest<fuchsia::component::runner::ComponentController> controller,
    async_dispatcher_t* dispatcher, LocalComponentFactory local_component_factory,
    fuchsia::component::runner::ComponentStartInfo start_info,
    fit::function<void()> on_instance_exit)
    : binding_(this), starting_(false), started_(false), on_exit_(std::move(on_instance_exit)) {
  ZX_COMPONENT_ASSERT_STATUS_OK("Bind ComponentController",
                                binding_.Bind(std::move(controller), dispatcher));
  // Perform the start command
  local_component_ = local_component_factory();
  fdio_ns_t* ns;
  ZX_COMPONENT_ASSERT_STATUS_OK("CreateHandlesFromStartInfo", fdio_ns_create(&ns));
  for (auto& entry : *start_info.mutable_ns()) {
    ZX_COMPONENT_ASSERT_STATUS_OK(
        "CreateHandlesFromStartInfo",
        fdio_ns_bind(ns, entry.path().c_str(), entry.mutable_directory()->TakeChannel().release()));
  }
  ZX_COMPONENT_ASSERT_STATUS_OK(
      "Initialize namespace and outgoing directory",
      local_component_->Initialize(ns, start_info.mutable_outgoing_dir()->TakeChannel(), dispatcher,
                                   [this](zx_status_t status) { Exit(status); }));
}

void LocalComponentInstance::Start() {
  starting_ = true;
  local_component_->OnStart();
  started_ = true;
  starting_ = false;
  if (pending_exit_status_) {
    // `ComponentInstance::Exit()` (which calls the
    // `ComponentInstance::on_exit_` callback) must not be called before the
    // `on_start_` callback completes.
    //
    // A LocalComponentImplBase may call `Exit()` during the
    // `LocalComponentImplBase::OnStart()` method. This is a legitimate use case, if
    // the component can complete its work via synchronous calls. See the
    // `RoutesProtocolToLocalComponentSync` test in `realm_builder_test.cc`, for
    // example. This test's component uses an `EchoSyncPtr` to invoke a client
    // request and get a response, before the `OnStart()` method completes.
    // Since the work is done, the client component is safe to terminate, by
    // calling `Exit()`.
    //
    // Note that calling `Exit()` before `on_start_` saves the provided status
    // (see above), but delays the call to `LocalComponentInstance::Exit()`
    // until here, after `on_start_` has completed.
    Exit(*pending_exit_status_);
    // `this` may now be invalid
  }
}

bool LocalComponentInstance::IsRunning() {
  return (starting_ || started_) && !pending_exit_status_ && binding_.is_bound();
}

void LocalComponentInstance::Stop() {
  local_component_->OnStop();

  Exit(ZX_OK);
  // The component should exit the loop, if any, or should have already
  // terminated. When it terminates, it should close the ComponentController.
  // If it doesn't, component manager will call Kill(), which can force close
  // the ComponentController.
}

void LocalComponentInstance::handle_unknown_method(uint64_t ordinal, bool has_response) {}
void LocalComponentInstance::Exit(zx_status_t epitaph_value) {
  // Don't actually exit during Start.  Start will check pending_exit_status_ at the end.
  if (!started_) {
    pending_exit_status_ = epitaph_value;
    return;
  }

  if (binding_.is_bound()) {
    binding_.Close(epitaph_value);
  }
  if (on_exit_) {
    // If on_exit is not set, this is a LocalComponent* type, which does not
    // support exiting. Don't close the binding while the component is still
    // running.
    auto on_exit = std::move(on_exit_);
    on_exit_ = nullptr;
    on_exit();
  }
}

LocalComponentRunner::LocalComponentRunner(LocalComponents components,
                                           async_dispatcher_t* dispatcher)
    : ready_components_(std::move(components)), binding_(this), dispatcher_(dispatcher) {}

fidl::InterfaceHandle<fuchsia::component::runner::ComponentRunner>
LocalComponentRunner::NewBinding() {
  return binding_.NewBinding(dispatcher_);
}

void LocalComponentRunner::Start(
    fuchsia::component::runner::ComponentStartInfo start_info,
    fidl::InterfaceRequest<fuchsia::component::runner::ComponentController> controller) {
  ZX_ASSERT_MSG(start_info.has_program(), "Component manager sent start_info without program");
  std::string const name = ExtractLocalComponentName(start_info.program());

  auto local_component_factory = [this, name]() { return SetComponentToRunning(name); };
  auto on_exit = [this, name]() { SetComponentToReady(name); };

  running_component_instances_[name] = std::make_unique<LocalComponentInstance>(
      std::move(controller), dispatcher_, std::move(local_component_factory), std::move(start_info),
      std::move(on_exit));
  // Start the component instance.
  running_component_instances_[name]->Start();
}

std::unique_ptr<LocalComponentImplBase> LocalComponentRunner::SetComponentToRunning(
    std::string name) {
  ZX_ASSERT_MSG(ready_components_.find(name) != ready_components_.cend(),
                "Component manager requested a named LocalComponent that is unregistered, already "
                "running, or not restartable. Component name: %s",
                name.c_str());
  running_components_[name] = std::move(ready_components_[name]);
  ZX_ASSERT_MSG(ready_components_.erase(name) == 1, "ready component not erased");
  return std::get<LocalComponentFactory>(running_components_[name])();
}

void LocalComponentRunner::SetComponentToReady(std::string name) {
  ZX_ASSERT_MSG(running_components_.find(name) != ready_components_.cend(),
                "Component manager requested a named LocalComponent that is unregistered, already "
                "running, or not restartable. Component name: %s",
                name.c_str());
  // return the factory back to the list of components that can be restarted
  ready_components_[name] = std::move(running_components_[name]);
  ZX_ASSERT_MSG(running_components_.erase(name) == 1, "running component not erased");
  // Drop the ComponentInstance. This also causes the ComponentController
  // and LocalComponentImplBase to be dropped.
  ZX_ASSERT_MSG(running_component_instances_.erase(name) == 1,
                "running component instance not erased");
}

bool LocalComponentRunner::ContainsReadyComponent(std::string name) const {
  return ready_components_.find(name) != ready_components_.cend();
}

std::unique_ptr<LocalComponentRunner> LocalComponentRunner::Builder::Build(
    async_dispatcher_t* dispatcher) {
  return std::make_unique<LocalComponentRunner>(std::move(components_), dispatcher);
}

void LocalComponentRunner::Builder::Register(std::string name, LocalComponentKind mock) {
  ZX_ASSERT_MSG(!Contains(name), "Local component with same name being added: %s", name.c_str());
  components_[name] = std::move(mock);
}

bool LocalComponentRunner::Builder::Contains(std::string name) const {
  return components_.find(name) != components_.cend();
}

}  // namespace internal
}  // namespace component_testing
