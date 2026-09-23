// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "runtime-module.h"

#include <lib/ld/decoded-module.h>
#include <lib/ld/dl-phdr-info.h>

#include <algorithm>

namespace dl {

bool RuntimeModule::ReifyModuleTree(Diagnostics& diag) {
  if (!module_tree_.is_empty()) {
    return true;
  }

  auto enqueue = [this, &diag](RuntimeModule* module) -> bool {
    return module_tree_.push_back(diag, "module dependencies list", module);
  };

  // This module is the root and always the first entry in the module tree.
  if (!enqueue(this)) {
    return false;
  }

  auto on_queue = [this](const RuntimeModule& module) -> bool {
    return std::ranges::any_of(module_tree(), module.name().equal_to());
  };

  // Skip if the module has already been enqueued, preventing circular deps.
  auto enqueue_unique = [enqueue, on_queue](RuntimeModule* module) {
    return on_queue(*module) || enqueue(module);
  };

  // Build the BFS-ordered queue: this uses an indexed-for-loop to iterate
  // over the list since it can't be invalidated as modules are enqueued.
  for (size_t i = 0; i < module_tree_.size(); ++i) {
    // Enqueue the first-level dependencies of the current module.
    if (!std::ranges::all_of(module_tree_[i]->direct_deps(), enqueue_unique)) {
      return false;
    }
  }

  return true;
}

// TODO(https://fxbug.dev/382527519): support synchronization with initializers.
void RuntimeModule::InitializeModuleTree() {
  // If this module is already initialized, then all its dependencies are
  // initialized, so return early.
  if (initialized_) {
    return;
  }

  // Initializers are run in reverse-order of the dependency tree, so that init
  // funcs for dependencies are run before the init funcs of dependent modules.
  // The init functions for this module will be run last.
  std::ranges::for_each(std::ranges::reverse_view(module_tree()), &RuntimeModule::Initialize);
}

void RuntimeModule::Initialize() {
  // If this module's init functions have already run, don't run them again.
  if (initialized_) {
    return;
  }
  initialized_ = true;
  module().init.CallInit(load_bias());
}

}  // namespace dl
