// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/test-elf-object.h"

#include <cassert>
#include <unordered_map>

namespace ld::testing {
namespace {

// The only way to create a TestElfLoadSet is the constructor implemented here.
// The only constructor calls are static constructors, so it inserts every one
// into this map for TestElfLoadSet::Get to find quickly.  Static constructors
// could be avoided by using a special-section link-time array, but it doesn't
// seem worth the complexity of that for test code.

using SetMap = std::unordered_map<elfldltl::Soname<>, const TestElfLoadSet&>;

// The global here is used from static constructors, and there is no guarantee
// about the order of static constructors across TUs.  So avoid a static
// constructor for the global by hiding it in a lazy-initializing function.
SetMap& GetMap() {
  static SetMap map;
  return map;
}

}  // namespace

TestElfLoadSet::TestElfLoadSet(elfldltl::Soname<> name, TestElfObjectList objects)
    : objects_{objects} {
  assert(!objects_.empty());
  [[maybe_unused]] auto [it, placed] = GetMap().try_emplace(name, *this);
  assert(placed);
  assert(std::addressof(it->second) == this);
}

const TestElfLoadSet* TestElfLoadSet::Get(elfldltl::Soname<> name) {
  SetMap& map = GetMap();
  if (auto it = map.find(name); it != map.end()) {
    return &it->second;
  }
  [[unlikely]] return nullptr;
}

TestElfLoadSet::SonameMap TestElfLoadSet::MakeSonameMap() const {
  SonameMap map;
  for (const TestElfObject* object : objects_) {
    if (!object->soname.empty()) {
      map.try_emplace(object->soname, *object);
    }
  }
  return map;
}

}  // namespace ld::testing
