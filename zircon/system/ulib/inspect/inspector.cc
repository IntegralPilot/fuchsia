// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fpromise/promise.h>
#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/heap.h>
#include <lib/inspect/cpp/vmo/state.h>
#include <lib/inspect/cpp/vmo/types.h>

using inspect::internal::Heap;
using inspect::internal::State;

namespace inspect {

namespace {
const InspectSettings kDefaultInspectSettings = {.maximum_size = 256 * 1024};
}  // namespace

Inspector::Inspector() : Inspector(kDefaultInspectSettings) {}

Inspector::Inspector(const InspectSettings& settings)
    : ctx_{
          .root_ = std::make_shared<Node>(),
          .state_ = nullptr,
          .value_list_ = std::make_shared<ValueList>(),
          .value_mutex_ = std::make_shared<std::mutex>(),
      } {
  if (settings.maximum_size == 0) {
    return;
  }

  ctx_.state_ = State::CreateWithSize(settings.maximum_size);
  if (!ctx_.state_) {
    return;
  }

  *ctx_.root_ = ctx_.state_->CreateRootNode();
}

Inspector::Inspector(zx::vmo vmo)
    : ctx_{
          .root_ = std::make_shared<Node>(),
          .state_ = nullptr,
          .value_list_ = std::make_shared<ValueList>(),
          .value_mutex_ = std::make_shared<std::mutex>(),
      } {
  size_t size;

  zx_status_t status;
  if (ZX_OK != (status = vmo.get_size(&size))) {
    return;
  }

  if (size == 0) {
    // VMO cannot be zero size.
    return;
  }

  // Decommit all pages, reducing memory usage of the VMO and zeroing it.
  if (ZX_OK != (status = vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, size, nullptr, 0))) {
    return;
  }

  ctx_.state_ = State::Create(std::make_unique<Heap>(std::move(vmo)));
  if (!ctx_.state_) {
    return;
  }

  *ctx_.root_ = ctx_.state_->CreateRootNode();
}

WeakInspector Inspector::AsWeak() const {
  return WeakInspector(internal::WeakContext{
      .root_ = ctx_.root_,
      .state_ = ctx_.state_,
      .value_list_ = ctx_.value_list_,
      .value_mutex_ = ctx_.value_mutex_,
  });
}

Inspector WeakInspector::lock() const {
  return Inspector(internal::StrongContext{.root_ = ctx_.root_.lock(),
                                           .state_ = ctx_.state_.lock(),
                                           .value_list_ = ctx_.value_list_.lock(),
                                           .value_mutex_ = ctx_.value_mutex_.lock()});
}

std::optional<zx::vmo> Inspector::FrozenVmoCopy() const {
  if (!ctx_.state_) {
    return {};
  }

  return ctx_.state_->FrozenVmoCopy();
}

zx::vmo Inspector::DuplicateVmo() const {
  zx::vmo ret;

  if (ctx_.state_) {
    ctx_.state_->DuplicateVmo(&ret);
  }

  return ret;
}

std::optional<zx::vmo> Inspector::CopyVmo() const {
  zx::vmo ret;

  if (!ctx_.state_ || !ctx_.state_->Copy(&ret)) {
    return {};
  }

  return {std::move(ret)};
}

std::optional<std::vector<uint8_t>> Inspector::CopyBytes() const {
  std::vector<uint8_t> ret;
  if (!ctx_.state_ || !ctx_.state_->CopyBytes(&ret)) {
    return {};
  }

  return {std::move(ret)};
}

InspectStats Inspector::GetStats() const {
  if (!ctx_.state_) {
    return InspectStats{};
  }
  return ctx_.state_->GetStats();
}

Node& Inspector::GetRoot() const { return *ctx_.root_; }

std::vector<std::string> Inspector::GetChildNames() const {
  if (!ctx_.state_) {
    return {};
  }
  return ctx_.state_->GetLinkNames();
}

fpromise::promise<Inspector> Inspector::OpenChild(const std::string& child_name) const {
  if (!ctx_.state_) {
    return fpromise::make_result_promise<Inspector>(fpromise::error());
  }
  return ctx_.state_->CallLinkCallback(child_name);
}

void Inspector::AtomicUpdate(AtomicUpdateCallbackFn callback) {
  GetRoot().AtomicUpdate(std::move(callback));
}

namespace {
// The metric node name, as exposed by the stats node.
const char* FUCHSIA_INSPECT_STATS = "fuchsia.inspect.Stats";
const char* CURRENT_SIZE_KEY = "current_size";
const char* MAXIMUM_SIZE_KEY = "maximum_size";
const char* UTILIZATION_PER_TEN_K_KEY = "utilization_per_ten_k";
const char* TOTAL_DYNAMIC_CHILDREN_KEY = "total_dynamic_children";
const char* ALLOCATED_BLOCKS_KEY = "allocated_blocks";
const char* DEALLOCATED_BLOCKS_KEY = "deallocated_blocks";
const char* FAILED_ALLOCATIONS_KEY = "failed_allocations";
}  // namespace

void Inspector::CreateStatsNode() {
  GetRoot().CreateLazyNode(
      FUCHSIA_INSPECT_STATS,
      [weak_insp = AsWeak()] {
        auto insp = weak_insp.lock();
        if (!insp) {
          return fpromise::make_ok_promise(Inspector());
        }
        auto stats = insp.GetStats();
        Inspector stats_insp;
        stats_insp.GetRoot().CreateUint(CURRENT_SIZE_KEY, stats.size, &stats_insp);
        stats_insp.GetRoot().CreateUint(MAXIMUM_SIZE_KEY, stats.maximum_size, &stats_insp);
        if (stats.maximum_size > 0) {
          stats_insp.GetRoot().CreateUint(UTILIZATION_PER_TEN_K_KEY,
                                          (stats.size * 10000) / stats.maximum_size, &stats_insp);
        }
        stats_insp.GetRoot().CreateUint(TOTAL_DYNAMIC_CHILDREN_KEY, stats.dynamic_child_count,
                                        &stats_insp);
        stats_insp.GetRoot().CreateUint(ALLOCATED_BLOCKS_KEY, stats.allocated_blocks, &stats_insp);
        stats_insp.GetRoot().CreateUint(DEALLOCATED_BLOCKS_KEY, stats.deallocated_blocks,
                                        &stats_insp);
        stats_insp.GetRoot().CreateUint(FAILED_ALLOCATIONS_KEY, stats.failed_allocations,
                                        &stats_insp);
        return fpromise::make_ok_promise(stats_insp);
      },
      this);
}

namespace internal {
std::shared_ptr<State> GetState(const Inspector* inspector) { return inspector->ctx_.state_; }
}  // namespace internal

}  // namespace inspect
