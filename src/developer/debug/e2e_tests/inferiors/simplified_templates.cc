// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is for testing various combinations of templates, and is used for the
// simple_template_names e2e test script. Specifically, this file is compiled with the
// -gsimple-template-names flag, which saves large amounts of space in debug builds, but requires us
// to do more work when indexing symbols to get the proper names indexed.

#include <array>
#include <iostream>
#include <numeric>
#include <sstream>
#include <vector>

#include "src/lib/debug/debug.h"

// Free standing templatized function.
template <typename T>
std::string ToString(T t) {
  std::stringstream o;
  o << t;
  return o.str();
}

// Typical class template.
template <typename T>
class MyClass {
 public:
  explicit MyClass(T t) : my_t_(t) {}
  void Print() const { std::cout << my_t_ << std::endl; }

 private:
  T my_t_;
};

// Template specialization.
template <>
class MyClass<int> {
 public:
  explicit MyClass(int n) : my_num_(n) {}
  int Add(int n) const { return my_num_ + n; }

 private:
  int my_num_;
};

// More than one template type.
template <typename T, typename U>
class MyDoubleTemplate {
 public:
  explicit MyDoubleTemplate(T t, U u) : my_t_(t), my_u_(u) {}
  void Print() const { std::cout << "my_t_ = " << my_t_ << " my_u_ = " << my_u_ << std::endl; }

 private:
  T my_t_;
  U my_u_;
};

template <int k>
class MyTemplateValueClass {
 public:
  MyTemplateValueClass() = default;
  void Print() const { std::cout << "my_k_ = " << my_k_; }

 private:
  const int my_k_ = k;
};

template <char c>
class MyTemplateValueClassChar {
 public:
  MyTemplateValueClassChar() = default;
  void Print() const { std::cout << "my_c_ = " << my_c_; }

 private:
  const char my_c_ = c;
};

template <bool b>
struct MyStruct {};

int main() {
  std::cout << ToString(1234) << std::endl;
  std::cout << ToString(3.14159) << std::endl;

  MyClass<char> myclass('c');
  MyClass<double> mcd(48.3);
  MyClass<int> myi(12);
  MyDoubleTemplate<int, char> my_double_template(5, 'c');
  MyDoubleTemplate<double, uint64_t> mdtd64(12.4, 0x43110);
  MyTemplateValueClass<5> mtvc;
  MyTemplateValueClassChar<'f'> mtvcc;
  MyStruct<true> foo;
  MyStruct<false> bar;

  debug::BreakIntoDebuggerIfAttached();

  myclass.Print();
  mcd.Print();
  std::cout << myi.Add(6) << std::endl;
  my_double_template.Print();
  mdtd64.Print();
  mtvc.Print();
  mtvcc.Print();
  (void)foo;
  (void)bar;
}
