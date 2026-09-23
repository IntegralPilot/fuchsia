// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdint>
#include <cstdio>

#pragma pack(1)
struct Parent1 {
  uint8_t field0 : 5;
};

struct Parent2 {
  int8_t field1 : 1;
  int8_t field2 : 7;
};

struct Child : public Parent1, public Parent2 {
  uint8_t field3;
  uint8_t field4 : 3;
  uint16_t field5 : 9;
  uint8_t field6 : 4;
};

uint8_t data[] = {2, 0xAA, 8, 0x13, 0x7F};
static_assert(sizeof(Child) == sizeof(data));

//  data: 01000000 01010101 00010000 11001000 11111110
//        \___/    |\_____/ \______/ \_/\________/\__/
// field:   0      1   2       3      4     5       6
// value:   2      0  -43      8      3    482      7
Child* p = reinterpret_cast<Child*>(data);

int main() {
  printf("field0 = %d\n", p->field0);
  printf("field1 = %d\n", p->field1);
  printf("field2 = %d\n", p->field2);
  printf("field3 = %d\n", p->field3);
  printf("field4 = %d\n", p->field4);
  printf("field5 = %d\n", p->field5);
  printf("field6 = %d\n", p->field6);
}
