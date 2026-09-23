// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "profile_manager.h"

#include <lib/fdio/directory.h>
#include <lib/zx/profile.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/threads.h>

#include <memory>
#include <thread>
#include <unordered_map>
#include <utility>

#include "src/lib/fxl/strings/string_printf.h"

namespace hwstress {

zx::unowned<zx::thread> HandleFromThread(std::thread* thread) {
  // The native handle is a thrd_t. We don't currently provide any nice way to convert this, other
  // than this ugly cast.
  zx_handle_t handle = thrd_get_zx_handle(static_cast<thrd_t>(thread->native_handle()));
  return zx::unowned<zx::thread>(handle);
}

std::unique_ptr<ProfileManager> ProfileManager::CreateFromEnvironment() {
  zx::channel channel0, channel1;
  zx_status_t status;

  status = zx::channel::create(0u, &channel0, &channel1);
  if (status != ZX_OK) {
    return nullptr;
  }

  status = fdio_service_connect(
      (std::string("/svc/") + fuchsia::kernel::ProfileResource::Name_).c_str(), channel0.release());
  if (status != ZX_OK) {
    return nullptr;
  }

  zx::resource profile_resource;
  fuchsia::kernel::ProfileResource_SyncProxy proxy(std::move(channel1));
  status = proxy.Get(&profile_resource);
  if (status != ZX_OK) {
    return nullptr;
  }

  return std::make_unique<ProfileManager>(std::move(profile_resource));
}

ProfileManager::ProfileManager(zx::resource profile_resource)
    : profile_resource_(std::move(profile_resource)) {}

zx_status_t ProfileManager::SetThreadAffinity(const zx::thread& thread, uint32_t mask) {
  return CreateAndApplyProfile<uint32_t>(
      &affinity_profiles_, mask,
      [this](uint32_t mask) -> zx::result<zx::profile> {
        zx::profile profile;
        zx_profile_info_t info = {
            .flags = ZX_PROFILE_INFO_FLAG_CPU_MASK,
            .cpu_affinity_mask = {mask},
        };
        zx_status_t status = zx::profile::create(profile_resource_, 0u, &info, &profile);
        if (status != ZX_OK) {
          return zx::error(status);
        }
        return zx::ok(std::move(profile));
      },
      thread);
}

zx_status_t ProfileManager::SetThreadAffinity(std::thread* thread, uint32_t mask) {
  return SetThreadAffinity(*HandleFromThread(thread), mask);
}

zx_status_t ProfileManager::SetThreadPriority(const zx::thread& thread, uint32_t priority) {
  return CreateAndApplyProfile<uint32_t>(
      &priority_profiles_, priority,
      [this](uint32_t priority) -> zx::result<zx::profile> {
        zx::profile profile;
        zx_profile_info_t info = {
            .flags = ZX_PROFILE_INFO_FLAG_PRIORITY,
            .priority = static_cast<int32_t>(priority),
        };
        zx_status_t status = zx::profile::create(profile_resource_, 0u, &info, &profile);
        if (status != ZX_OK) {
          return zx::error(status);
        }
        return zx::ok(std::move(profile));
      },
      thread);
}

zx_status_t ProfileManager::SetThreadPriority(std::thread* thread, uint32_t priority) {
  return SetThreadPriority(*HandleFromThread(thread), priority);
}

}  // namespace hwstress
