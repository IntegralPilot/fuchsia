// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/dso/pointerinjector_registry.h"

#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/types.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include <array>
#include <mutex>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/fidl_array_cast.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/math.h"
#include "zircon/errors.h"

namespace scenic_impl::input_dso {

using fuchsia_ui_pointerinjector::wire::Context;
using fuchsia_ui_pointerinjector::wire::DeviceType;
using fuchsia_ui_pointerinjector::wire::DispatchPolicy;
using fuchsia_ui_pointerinjector::wire::Target;
using scenic_impl::input::Extents;

namespace {

bool IsValidConfig(const fuchsia_ui_pointerinjector::wire::Config& config) {
  if (!config.has_device_id() || !config.has_device_type() || !config.has_context() ||
      !config.has_target() || !config.has_viewport() || !config.has_dispatch_policy()) {
    FX_LOGS(ERROR) << "InjectorRegistry::Register : Argument |config| is incomplete.";
    return false;
  }

  const auto device_type = config.device_type();
  if (device_type != DeviceType::kTouch) {
    FX_LOGS(ERROR) << "InjectorRegistry::Register : Unknown DeviceType.";
    return false;
  }

  const auto dispatch_policy = config.dispatch_policy();
  if (device_type == DeviceType::kTouch) {
    if (dispatch_policy != DispatchPolicy::kExclusiveTarget &&
        dispatch_policy != DispatchPolicy::kTopHitAndAncestorsInTarget) {
      FX_LOGS(ERROR)
          << "InjectorRegistry::Register : DeviceType::TOUCH with mismatched dispatch policy.";
      return false;
    }
  } else {
    FX_NOTREACHED();
  }

  if (!config.context().is_view() || !config.target().is_view()) {
    FX_LOGS(ERROR) << "InjectorRegistry::Register : Argument |config.context| or |config.target| "
                      "is not a view. Only views are supported.";
    return false;
  }

  if (Injector::IsValidViewport(config.viewport()) != ZX_OK) {
    // Errors printed in IsValidViewport. Just return result here.
    return false;
  }

  return true;
}

}  // namespace

PointerinjectorRegistry::PointerinjectorRegistry(
    async_dispatcher_t* input_dispatcher,
    std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
    TouchInjectFunc inject_touch_exclusive, TouchInjectFunc inject_touch_hit_tested,
    inspect::Node inspect_node)
    : inject_touch_exclusive_(std::move(inject_touch_exclusive)),
      inject_touch_hit_tested_(std::move(inject_touch_hit_tested)),
      snapshot_holder_(std::move(snapshot_holder)),
      input_dispatcher_(input_dispatcher),
      inspect_node_(std::move(inspect_node)) {
  FX_DCHECK(input_dispatcher);
}

void PointerinjectorRegistry::Bind(fdf::Channel channel) {
  utils::CheckIsOnInputThread();
  injector_registry_.AddBinding(
      reinterpret_cast<fdf_dispatcher_t*>(input_dispatcher_),
      fdf::ServerEnd<fuchsia_ui_pointerinjector_dso::Registry>(std::move(channel)), this,
      [](fidl::UnbindInfo unbind) {});
}

void PointerinjectorRegistry::Register(RegisterRequestView request, fdf::Arena& arena,
                                       RegisterCompleter::Sync& completer) {
  TRACE_DURATION("input", "PointerinjectorRegistry::Register");
  auto reply_defer = fit::defer([&arena, &completer] { completer.buffer(arena).Reply(); });

  auto& config = request->config;
  auto& injector = request->injector;

  if (!IsValidConfig(config)) {
    // Errors printed inside IsValidConfig. Just return here.
    injector.reset();  // no epitaph supported by driver transport
    return;
  }

  // Check connectivity here, since injector doesn't have access to it.
  const zx_koid_t context_koid = utils::ExtractKoid(config.context().view());
  const zx_koid_t target_koid = utils::ExtractKoid(config.target().view());
  if (context_koid == ZX_KOID_INVALID || target_koid == ZX_KOID_INVALID) {
    FX_LOGS(ERROR) << "InjectorRegistry::Register : Argument |config.context| or |config.target| "
                      "was invalid.";
    injector.reset();  // no epitaph supported by driver transport
    return;
  }
  auto snapshot = snapshot_holder_->GetSnapshot();

  if (!snapshot->IsDescendant(target_koid, context_koid)) {
    FX_LOGS(ERROR) << "InjectorRegistry::Register : Argument |config.context| must be connected to "
                      "the Scene, and |config.target| must be a descendant of |config.context|";
    injector.reset();  // no epitaph supported by driver transport
    return;
  }

  const InjectorId id = ++last_injector_id_;
  InjectorSettings settings{.dispatch_policy = config.dispatch_policy(),
                            .device_id = config.device_id(),
                            .device_type = config.device_type(),
                            .context_koid = context_koid,
                            .target_koid = target_koid};
  Viewport viewport{
      .extents = Extents(config.viewport().extents()),
      .context_from_viewport_transform = utils::ColumnMajorMat3ArrayToMat4(
          utils::ReinterpretFidlArrayAsStdArray(config.viewport().viewport_to_context_transform())),
  };

  fit::function<void()> on_channel_closed = [this, id] { injectors_.erase(id); };

  if (settings.device_type == fuchsia_ui_pointerinjector::DeviceType::kTouch) {
    const auto [_, success] = injectors_.emplace(
        id,
        std::make_unique<TouchInjector>(
            input_dispatcher_, snapshot_holder_,
            inspect_node_.CreateChild(inspect_node_.UniqueName("touch-injector-")),
            std::move(settings), viewport, std::move(injector),
            /*inject=*/
            [&inject_func = settings.dispatch_policy ==
                                    fuchsia_ui_pointerinjector::DispatchPolicy::kExclusiveTarget
                                ? inject_touch_exclusive_
                                : inject_touch_hit_tested_](
                InternalTouchEvent event, StreamId stream_id, const view_tree::Snapshot& snapshot) {
              TRACE_DURATION("input", "TouchInjector::inject_");
              inject_func(std::move(event), stream_id, snapshot);
            },
            std::move(on_channel_closed)));
    FX_CHECK(success) << "Injector already exists.";
  } else {
    FX_NOTREACHED();
  }
}

}  // namespace scenic_impl::input_dso
