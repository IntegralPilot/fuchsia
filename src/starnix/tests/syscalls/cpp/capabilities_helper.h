// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_CAPABILITIES_HELPER_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_CAPABILITIES_HELPER_H_

namespace test_helper {

// Drops all capabilities from the effective, permitted, and inheritable sets.
void DropAllCapabilities();

// Checks if a capability is in the effective set.
bool HasCapability(int cap);

// Unsets a capability from the effective set.
void UnsetCapability(int cap);

// Checks whether a capability is in the thread's effective set.
bool HasCapabilityEffective(int cap);

// Checks whether a capability is in the thread's permitted set.
bool HasCapabilityPermitted(int cap);

// Checks whether a capability is in the thread's inheritable set.
bool HasCapabilityInheritable(int cap);

// Checks whether a capability is in the thread's ambient set.
bool HasCapabilityAmbient(int cap);

// Checks whether a capability is in the thread's bounding set.
bool HasCapabilityBounding(int cap);

// Removes a capability from the thread's effective set.
void UnsetCapabilityEffective(int cap);

// Removes a capability from the thread's permitted set.
void UnsetCapabilityPermitted(int cap);

// Removes a capability from the thread's inheritable set.
void UnsetCapabilityInheritable(int cap);

// Removes a capability from the thread's ambient set.
void UnsetCapabilityAmbient(int cap);

// Removes a capability from the thread's bounded set.
void UnsetCapabilityBounding(int cap);

// Drops all capabilities from the ambient set.
void DropAllAmbientCapabilities();

// Sets a capability in the thread's effective set.
void SetCapabilityEffective(int cap);

// Sets a capability in the thread's permitted set.
void SetCapabilityPermitted(int cap);

// Sets a capability in the thread's inheritable set.
void SetCapabilityInheritable(int cap);

// Sets a capability in the thread's ambient set.
void SetCapabilityAmbient(int cap);

// Sets a capability in the thread's bounding set.
void SetCapabilityBounding(int cap);

}  // namespace test_helper

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_CAPABILITIES_HELPER_H_
