// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/test/common/vk/vk_debug_utils_message_collector.h"

#include <gtest/gtest.h>

namespace escher::test {
namespace impl {

uint32_t VkDebugUtilsMessageCollector::HandleDebugUtilsMessage(
    VkDebugUtilsMessageSeverityFlagBitsEXT message_severity,
    VkDebugUtilsMessageTypeFlagsEXT message_types,
    const VkDebugUtilsMessengerCallbackDataEXT *callback_data, void *user_data) {
  vk::ObjectType object_type = callback_data->objectCount == 0
                                   ? vk::ObjectType::eUnknown
                                   : vk::ObjectType(callback_data->pObjects[0].objectType);
  uint64_t object_handle =
      callback_data->objectCount == 0 ? 0ull : callback_data->pObjects[0].objectHandle;

  auto &debug_utils_messages =
      static_cast<VkDebugUtilsMessageCollector *>(user_data)->debug_utils_messages_;
  debug_utils_messages.emplace_back(
      VkDebugUtilsMessage{.severity = vk::DebugUtilsMessageSeverityFlagsEXT(message_severity),
                          .types = vk::DebugUtilsMessageTypeFlagsEXT(message_types),
                          .object_type = object_type,
                          .object = object_handle,
                          .layer_prefix = callback_data->pMessageIdName,
                          .message_code = callback_data->messageIdNumber,
                          .message = callback_data->pMessage});
  return false;
}

bool VkDebugUtilsMessageCollector::PrintDebugUtilsMessagesWithFlags(
    const vk::DebugUtilsMessageSeverityFlagsEXT &severity,
    const vk::DebugUtilsMessageTypeFlagsEXT &types, const char *file, size_t line) {
  const std::vector<VkDebugUtilsMessage> debug_utils_messages_with_flags =
      DebugUtilsMessagesWithFlag(severity, types);
  for (const auto &debug_utils_message : debug_utils_messages_with_flags) {
    GTEST_MESSAGE_AT_(file, line, debug_utils_message.ErrorMessage().c_str(),
                      ::testing::TestPartResult::kNonFatalFailure);
  }
  return !debug_utils_messages_with_flags.empty();
}

bool VkDebugUtilsMessageCollector::PrintDebugUtilsMessagesOnFalsePredicate(
    const vk::DebugUtilsMessageSeverityFlagsEXT &severity,
    const vk::DebugUtilsMessageTypeFlagsEXT &types, size_t num_threshold,
    const std::function<bool(size_t, size_t)> &pred, const char *file, size_t line) const {
  const std::vector<VkDebugUtilsMessage> debug_utils_messages_with_flags =
      DebugUtilsMessagesWithFlag(severity, types);
  bool result = true;
  if (!pred(debug_utils_messages_with_flags.size(), num_threshold)) {
    for (const auto &debug_utils_message : debug_utils_messages_with_flags) {
      GTEST_MESSAGE_AT_(file, line, debug_utils_message.ErrorMessage().c_str(),
                        ::testing::TestPartResult::kNonFatalFailure);
    }
    result = false;
  }
  return result;
}

std::vector<VkDebugUtilsMessageCollector::VkDebugUtilsMessage>
VkDebugUtilsMessageCollector::DebugUtilsMessagesWithFlag(
    const vk::DebugUtilsMessageSeverityFlagsEXT &severity,
    const vk::DebugUtilsMessageTypeFlagsEXT &types) const {
  std::vector<VkDebugUtilsMessage> result = {};
  std::copy_if(debug_utils_messages_.begin(), debug_utils_messages_.end(),
               std::back_inserter(result), [severity, types](const auto &message) {
                 return (message.severity & severity) && (message.types & types);
               });
  return result;
}

void VkDebugUtilsMessageCollector::RemoveDebugUtilsMessagesWithFlag(
    const vk::DebugUtilsMessageSeverityFlagsEXT &severity,
    const vk::DebugUtilsMessageTypeFlagsEXT &types) {
  auto end = std::remove_if(debug_utils_messages_.begin(), debug_utils_messages_.end(),
                            [severity, types](const VkDebugUtilsMessage &message) {
                              return (message.severity & severity) && (message.types & types);
                            });
  debug_utils_messages_.erase(end, debug_utils_messages_.end());
}

}  // namespace impl
}  // namespace escher::test
