// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//
//
//

#include "debug_utils.h"

//
//
//

#define DU_PFN_NAME(name_) pfn_##name_

#define DU_PFN_TYPE(name_) PFN_##name_

#define DU_PFN_DECL(name_) DU_PFN_TYPE(name_) DU_PFN_NAME(name_) = NULL

#define DU_PFN_INSTANCE_INIT(instance_, name_)                                                     \
  DU_PFN_NAME(name_) = (DU_PFN_TYPE(name_))vkGetInstanceProcAddr(instance_, #name_)

//
// The pfns default to NULL
//
DU_PFN_DECL(vkSetDebugUtilsObjectNameEXT);
DU_PFN_DECL(vkSetDebugUtilsObjectTagEXT);
DU_PFN_DECL(vkQueueBeginDebugUtilsLabelEXT);
DU_PFN_DECL(vkQueueEndDebugUtilsLabelEXT);
DU_PFN_DECL(vkQueueInsertDebugUtilsLabelEXT);
DU_PFN_DECL(vkCmdBeginDebugUtilsLabelEXT);
DU_PFN_DECL(vkCmdEndDebugUtilsLabelEXT);
DU_PFN_DECL(vkCmdInsertDebugUtilsLabelEXT);
DU_PFN_DECL(vkCreateDebugUtilsMessengerEXT);
DU_PFN_DECL(vkSubmitDebugUtilsMessageEXT);

//
// Initialize debug util instance extension pfns.
//

void
vk_debug_utils_init(VkInstance instance)
{
  DU_PFN_INSTANCE_INIT(instance, vkSetDebugUtilsObjectNameEXT);
  DU_PFN_INSTANCE_INIT(instance, vkSetDebugUtilsObjectTagEXT);
  DU_PFN_INSTANCE_INIT(instance, vkQueueBeginDebugUtilsLabelEXT);
  DU_PFN_INSTANCE_INIT(instance, vkQueueEndDebugUtilsLabelEXT);
  DU_PFN_INSTANCE_INIT(instance, vkQueueInsertDebugUtilsLabelEXT);
  DU_PFN_INSTANCE_INIT(instance, vkCmdBeginDebugUtilsLabelEXT);
  DU_PFN_INSTANCE_INIT(instance, vkCmdEndDebugUtilsLabelEXT);
  DU_PFN_INSTANCE_INIT(instance, vkCmdInsertDebugUtilsLabelEXT);
  DU_PFN_INSTANCE_INIT(instance, vkCreateDebugUtilsMessengerEXT);
  DU_PFN_INSTANCE_INIT(instance, vkSubmitDebugUtilsMessageEXT);
}

//
//
//
