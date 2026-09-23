// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <iostream>
#include <map>
#include <memory>
#include <span>
#include <string>
#include <string_view>
#include <unordered_set>
#include <vector>

class Parent {
 public:
  virtual ~Parent() = default;

  int a = 0;
};

class Child : public Parent {
 public:
  int b = 1;
};

int main() {
  std::shared_ptr<Parent> p = std::make_shared<Child>();
  std::unique_ptr<Parent> up = std::make_unique<Child>();
  std::string_view sv = "abc";
  std::map<std::string, int> map = {{"a", 1}, {"d", 4}, {"e", 5}, {"f", 6}};
  // TODO(https://fxbug.dev/461736288): Fix pretty-printing std::unordered_set.
  // std::unordered_set<int> set = {0, 1, 2};
  auto span = std::span<const char>(sv.begin() + 1, 2);

  std::vector<int> vals = {3, 4, 5, 6};

  for (auto it = vals.begin(); it != vals.end(); ++it) {
    std::cout << *it;
  }
  std::cout << "\n";

  std::cout << sv << p->a;
}
