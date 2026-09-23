// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_FACTORY_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_FACTORY_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>

#include "src/ui/scenic/lib/flatland/flatland_manager.h"

namespace flatland {

class FlatlandFactoryImpl : public fidl::Server<fuchsia_ui_composition::FlatlandFactory> {
 public:
  explicit FlatlandFactoryImpl(std::shared_ptr<FlatlandManager> flatland_manager);
  ~FlatlandFactoryImpl() override = default;

  // |fuchsia_ui_composition::FlatlandFactory|
  void CreateFlatland(CreateFlatlandRequest& request,
                      CreateFlatlandCompleter::Sync& completer) override;

  static bool IsValidConfig(const fuchsia_ui_composition::FlatlandConfig& config);
  static FlatlandConfig ToInternalConfig(const fuchsia_ui_composition::FlatlandConfig& config);

  fidl::ProtocolHandler<fuchsia_ui_composition::FlatlandFactory> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  std::shared_ptr<FlatlandManager> flatland_manager_;
  fidl::ServerBindingGroup<fuchsia_ui_composition::FlatlandFactory> bindings_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_FACTORY_H_
