// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_UTILS_ESCHER_PROVIDER_H_
#define SRC_UI_SCENIC_LIB_UTILS_ESCHER_PROVIDER_H_

#ifdef __Fuchsia__
#include <fidl/fuchsia.io/cpp/fidl.h>
#endif
#include <lib/sys/cpp/component_context.h>

#include "src/ui/lib/escher/escher.h"

#include <vulkan/vulkan.hpp>

namespace utils {

escher::EscherUniquePtr CreateEscher(sys::ComponentContext* app_context,
                                     const fidl::SyncClient<fuchsia_io::Directory>& pkg_dir);

VkBool32 HandleDebugUtilsMessage(VkDebugUtilsMessageSeverityFlagBitsEXT message_severity,
                                 VkDebugUtilsMessageTypeFlagsEXT message_types,
                                 const VkDebugUtilsMessengerCallbackDataEXT* callback_data,
                                 void* user_data);

}  // namespace utils

#endif  // SRC_UI_SCENIC_LIB_UTILS_ESCHER_PROVIDER_H_
