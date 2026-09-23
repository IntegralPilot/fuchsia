// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"

#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>

#include <linux/capability.h>
#include <linux/prctl.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace test_helper {

// Drops all capabilities from the effective, permitted, and inheritable sets.
void DropAllCapabilities() {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3] = {{0, 0, 0}};
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Checks if a capability is in the effective set.
bool HasCapability(int cap) { return HasCapabilityEffective(cap); }

// Unsets a capability from the effective set.
void UnsetCapability(int cap) { UnsetCapabilityEffective(cap); }

// Checks whether a capability is in the thread's effective set.
bool HasCapabilityEffective(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  return caps[CAP_TO_INDEX(cap)].effective & CAP_TO_MASK(cap);
}

// Checks whether a capability is in the thread's permitted set.
bool HasCapabilityPermitted(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  return caps[CAP_TO_INDEX(cap)].permitted & CAP_TO_MASK(cap);
}

// Checks whether a capability is in the thread's inheritable set.
bool HasCapabilityInheritable(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  return caps[CAP_TO_INDEX(cap)].inheritable & CAP_TO_MASK(cap);
}

bool HasCapabilityAmbient(int cap) {
  return SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, cap, 0, 0)) == 1;
}

bool HasCapabilityBounding(int cap) { return SAFE_SYSCALL(prctl(PR_CAPBSET_READ, cap)) == 1; }

// Removes a capability from the thread's effective set.
void UnsetCapabilityEffective(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  caps[CAP_TO_INDEX(cap)].effective &= ~CAP_TO_MASK(cap);
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Removes a capability from the thread's permitted set.
void UnsetCapabilityPermitted(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  caps[CAP_TO_INDEX(cap)].permitted &= ~CAP_TO_MASK(cap);
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Removes a capability from the thread's inheritable set.
void UnsetCapabilityInheritable(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  caps[CAP_TO_INDEX(cap)].inheritable &= ~CAP_TO_MASK(cap);
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Removes a capability from the thread's ambient set.
void UnsetCapabilityAmbient(int cap) {
  SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_LOWER, cap, 0, 0));
}

// Removes a capability from the thread's bounding set.
void UnsetCapabilityBounding(int cap) { SAFE_SYSCALL(prctl(PR_CAPBSET_DROP, cap)); }

// Sets a capability in the thread's effective set.
void SetCapabilityEffective(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  caps[CAP_TO_INDEX(cap)].effective |= CAP_TO_MASK(cap);
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Sets a capability in the thread's permitted set.
void SetCapabilityPermitted(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  caps[CAP_TO_INDEX(cap)].permitted |= CAP_TO_MASK(cap);
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Sets a capability in the thread's inheritable set.
void SetCapabilityInheritable(int cap) {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  SAFE_SYSCALL(syscall(SYS_capget, &header, &caps));
  caps[CAP_TO_INDEX(cap)].inheritable |= CAP_TO_MASK(cap);
  SAFE_SYSCALL(syscall(SYS_capset, &header, &caps));
}

// Sets a capability in the thread's ambient set.
void SetCapabilityAmbient(int cap) {
  SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE, cap, 0L, 0L));
}

void DropAllAmbientCapabilities() {
  SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0L, 0L, 0L));
}

}  // namespace test_helper
