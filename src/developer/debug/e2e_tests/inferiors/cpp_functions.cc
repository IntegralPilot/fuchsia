// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <iostream>
#include <vector>

namespace {
int SomeGlobal = 0;
char kHello[] = "Hello!";
const char* kCharStar = "eybdooG!";

void LeafNoArgs() {
  // Create a local variable so something happens to the stack.
  volatile int i = 10;
  SomeGlobal = i;
}

void NestedNoArgs() { LeafNoArgs(); }

void NestedTwiceNoArgs() { NestedNoArgs(); }

int ReturnGlobalPlusOne() { return SomeGlobal + 1; }

int* GetIntPointer() { return &SomeGlobal; }

float GetFloat() { return 3.14159f; }

const char* GetCharPtr() { return kHello; }

void PrintHello() { std::cout << "Hello! SomeGlobal = " << SomeGlobal << std::endl; }

int AddTwoInts(int left, int right) { return left + right; }

int AddIntPointers(int* lhs, int* rhs) { return AddTwoInts(*lhs, *rhs); }

void SwapPointedToValues(int* lhs, int* rhs) {
  int tmp = *lhs;
  *lhs = *rhs;
  *rhs = tmp;
}

void EchoCharPtr(const char* str) { std::cout << "Got " << str << "\n"; }

struct SomeStruct {
  int one;
  int two;
  std::vector<int> nums;
};

void DoSomeStuff(SomeStruct* s) {
  s->one++;
  s->two++;
  s->nums.pop_back();
}

}  // namespace

int main() {
  NestedTwiceNoArgs();
  PrintHello();
  std::cout << ReturnGlobalPlusOne() << "\n";
  std::cout << *GetIntPointer() << "\n";
  std::cout << GetFloat() << "\n";
  std::cout << GetCharPtr() << "\n";
  std::cout << AddTwoInts(1, 3) << "\n";

  int lhs = 7, rhs = 8;
  std::cout << AddIntPointers(&lhs, &rhs);

  SwapPointedToValues(&lhs, &rhs);

  EchoCharPtr(kHello);
  EchoCharPtr(kCharStar);

  SomeStruct s;
  s.one = 1;
  s.two = 2;
  s.nums = {3, 4, 5, 6};
  DoSomeStuff(&s);
}
