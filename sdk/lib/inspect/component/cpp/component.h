// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_INSPECT_COMPONENT_CPP_COMPONENT_H_
#define LIB_INSPECT_COMPONENT_CPP_COMPONENT_H_

#include <fidl/fuchsia.inspect/cpp/markers.h>
#include <lib/async/cpp/executor.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/inspect/component/cpp/tree_handler_settings.h>
#include <lib/inspect/cpp/health.h>
#include <lib/inspect/cpp/inspect.h>

#include <string>

namespace inspect {
/// Options for a published `ComponentInspector`.
///
/// The default constructor is acceptable for many components, and will cause a default
/// Inspector to be published via `fuchsia.inspect.InspectSink`, connected via the component's
/// default namespace, using default `TreeHandlerSettings`.
struct PublishOptions final {
  /// Optionally specify an existing `Inspector`.
  Inspector inspector = {};

  /// Specify how `fuchsia.inspect.Tree` should behave.
  TreeHandlerSettings tree_handler_settings = {};

  /// Provide a name to appear in the tree's Inspect Metadata. By default, it will be
  /// `fuchsia.inspect#DEFAULT_TREE_NAME
  std::optional<std::string> tree_name = std::nullopt;

  /// Provide a fidl::ClientEnd, useful if `fuchsia.inspect.InspectSink` is not in the root
  /// namespace or is renamed.
  std::optional<fidl::ClientEnd<fuchsia_inspect::InspectSink>> client_end = std::nullopt;
};

/// Options for publishing a VMO.
///
/// This spawns a fuchsia.inspect.Tree server whose content is this VMO.
/// It does not support lazy nodes.
///
/// The only service method is a live VMO. If you prefer to have copy semantics,
/// copy the VMO before serving. This is because copying the VMO could be expensive.
struct VmoOptions {
  /// Provide a name to appear in the tree's Inspect Metadata. By default, it will be
  /// `fuchsia.inspect#DEFAULT_TREE_NAME
  std::optional<std::string> tree_name = std::nullopt;

  /// Provide a fidl::ClientEnd, useful if `fuchsia.inspect.InspectSink` is not in the root
  /// namespace or is renamed.
  std::optional<fidl::ClientEnd<fuchsia_inspect::InspectSink>> client_end = std::nullopt;
};

/// Publish a VMO according to `opts`.
void PublishVmo(async_dispatcher_t* dispatcher, zx::vmo vmo, VmoOptions opts);

/// ComponentInspector is an instance of an Inspector that
/// serves its Inspect data via the fuchsia.inspect.Tree protocol.
///
/// Example:
///
/// ```
/// #include <lib/async-loop/cpp/loop.h>
/// #include <lib/async-loop/default.h>
/// #include <lib/inspect/component/cpp/component.h>
///
/// int main() {
///   using inspect::ComponentInspector;
///
///   async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
///   auto* dispatcher = loop.dispatcher();
///   auto inspector = ComponentInspector(dispatcher, {});
///
///   inspector.root().RecordInt("val1", 1);
///
///   inspector.Health().Ok();
///
///   loop.Run();
///   return 0;
/// }
/// ```
class ComponentInspector final {
 public:
  /// Construct a `ComponentInspector` with the provided `PublishOptions`.
  /// This `ComponentInspector` will be published via `fuchsia.inspect.InspectSink`.
  ComponentInspector(async_dispatcher_t* dispatcher, PublishOptions opts);

  ComponentInspector(ComponentInspector&&) = default;
  ComponentInspector& operator=(ComponentInspector&&) = default;

  /// Get the Inspector's root node.
  Node& root() { return inspector_.GetRoot(); }

  /// Get the wrapped Inspector.
  const Inspector& inspector() const { return inspector_; }
  Inspector& inspector() { return inspector_; }

  /// Gets the NodeHealth for this component.
  /// This method is not thread safe.
  NodeHealth& Health();

 private:
  ComponentInspector() = delete;
  ComponentInspector(const ComponentInspector&) = delete;

  Inspector inspector_;
  std::unique_ptr<NodeHealth> component_health_;
};
}  // namespace inspect

#endif  // LIB_INSPECT_COMPONENT_CPP_COMPONENT_H_
