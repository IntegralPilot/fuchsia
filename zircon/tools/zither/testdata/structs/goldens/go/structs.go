// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zither.structs (//zircon/tools/zither/testdata/structs/structs.test.fidl)
// by zither, a Fuchsia platform tool.

package structs

type Empty struct {
}

type Singleton struct {
	Value uint8 `json:"value"`
}

type Doubtleton struct {
	First  Singleton `json:"first"`
	Second Singleton `json:"second"`
}

type PrimitiveMembers struct {
	I64 int64  `json:"i64"`
	U64 uint64 `json:"u64"`
	I32 int32  `json:"i32"`
	U32 uint32 `json:"u32"`
	I16 int16  `json:"i16"`
	U16 uint16 `json:"u16"`
	I8  int8   `json:"i8"`
	U8  uint8  `json:"u8"`
	B   bool   `json:"b"`
}

type ArrayMembers struct {
	U8s           [10]uint8     `json:"u8s"`
	Singletons    [6]Singleton  `json:"singletons"`
	NestedArrays1 [20][10]uint8 `json:"nested_arrays1"`
	NestedArrays2 [3][2][1]int8 `json:"nested_arrays2"`
}

type Enum int32

const (
	EnumZero Enum = 0
	EnumOne  Enum = 1
)

type Bits uint16

const (
	BitsOne Bits = 1 << 0
	BitsTwo Bits = 1 << 1
)

type EnumAndBitsMembers struct {
	E Enum `json:"e"`
	B Bits `json:"b"`
}

// Struct with a one-line comment.
type StructWithOneLineComment struct {

	// Struct member with one-line comment.
	MemberWithOneLineComment uint32 `json:"member_with_one_line_comment"`

	// Struct member
	//     with a
	//         many-line
	//           comment.
	MemberWithManyLineComment bool `json:"member_with_many_line_comment"`
}

// Struct
//
//	with a
//	    many-line
//	      comment.
type StructWithManyLineComment struct {
	Member uint16 `json:"member"`
}
