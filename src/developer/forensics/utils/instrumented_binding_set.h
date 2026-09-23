// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_INSTRUMENTED_BINDING_SET_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_INSTRUMENTED_BINDING_SET_H_

#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/inspect/cpp/vmo/types.h>

#include <string>

#include "src/developer/forensics/utils/inspect_node_manager.h"

namespace forensics {

template <typename Interface>
class InstrumentedBindingSet {
 public:
  InstrumentedBindingSet(async_dispatcher_t* dispatcher, Interface* impl, InspectNodeManager* node,
                         const std::string& path);

  void AddBinding(::fidl::InterfaceRequest<Interface> request);

 private:
  async_dispatcher_t* dispatcher_;
  Interface* impl_;

  inspect::UintProperty current_num_connections_;
  inspect::UintProperty total_num_connections_;
  ::fidl::BindingSet<Interface> connections_;
};

template <typename Interface>
InstrumentedBindingSet<Interface>::InstrumentedBindingSet(async_dispatcher_t* dispatcher,
                                                          Interface* impl, InspectNodeManager* node,
                                                          const std::string& path)
    : dispatcher_(dispatcher),
      impl_(impl),
      current_num_connections_(node->Get(path).CreateUint("current_num_connections", 0)),
      total_num_connections_(node->Get(path).CreateUint("total_num_connections", 0)) {}

template <typename Interface>
void InstrumentedBindingSet<Interface>::AddBinding(::fidl::InterfaceRequest<Interface> request) {
  current_num_connections_.Add(1u);
  total_num_connections_.Add(1u);
  connections_.AddBinding(impl_, std::move(request), dispatcher_, [this](const zx_status_t status) {
    current_num_connections_.Subtract(1u);
  });
}

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_INSTRUMENTED_BINDING_SET_H_
