// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zither.experimental.zx.types (//zircon/tools/zither/testdata/experimental_zx_types/experimental_zx_types.test.fidl)
// by zither, a Fuchsia platform tool.

package types

import (
	"unsafe"
)

// 'a'
const CharConst byte = 97

const SizeConst uint = 100

const UintptrConst uintptr = 0x1234abcd5678ffff

type StructWithPrimitives struct {
	CharField    byte    `json:"char_field"`
	SizeField    uint    `json:"size_field"`
	UintptrField uintptr `json:"uintptr_field"`
}

type Uint8Alias = uint8

type StructWithPointers struct {
	U64ptr   *uint64        `json:"u64ptr"`
	Charptr  *byte          `json:"charptr"`
	Usizeptr *uint          `json:"usizeptr"`
	Byteptr  *uint8         `json:"byteptr"`
	Voidptr  unsafe.Pointer `json:"voidptr"`
	Aliasptr *Uint8Alias    `json:"aliasptr"`
}

type StructWithStringArrays struct {
	Str  [10]byte   `json:"str"`
	Strs [4][6]byte `json:"strs"`
}

type OverlayStructVariant struct {
	Value uint64 `json:"value"`
}

type OverlayWithEquallySizedVariantsDiscriminant uint64

const (
	OverlayWithEquallySizedVariantsDiscriminantA OverlayWithEquallySizedVariantsDiscriminant = 1
	OverlayWithEquallySizedVariantsDiscriminantB OverlayWithEquallySizedVariantsDiscriminant = 2
	OverlayWithEquallySizedVariantsDiscriminantC OverlayWithEquallySizedVariantsDiscriminant = 3
	OverlayWithEquallySizedVariantsDiscriminantD OverlayWithEquallySizedVariantsDiscriminant = 4
)

type OverlayWithEquallySizedVariants struct {
	Discriminant OverlayWithEquallySizedVariantsDiscriminant
	variant      [8]byte
}

func (o OverlayWithEquallySizedVariants) IsA() bool {
	return o.Discriminant == OverlayWithEquallySizedVariantsDiscriminantA
}
func (o *OverlayWithEquallySizedVariants) AsA() *uint64 {
	if !o.IsA() {
		return nil
	}
	return (*uint64)(unsafe.Pointer(&o.variant))
}

func (o OverlayWithEquallySizedVariants) IsB() bool {
	return o.Discriminant == OverlayWithEquallySizedVariantsDiscriminantB
}
func (o *OverlayWithEquallySizedVariants) AsB() *int64 {
	if !o.IsB() {
		return nil
	}
	return (*int64)(unsafe.Pointer(&o.variant))
}

func (o OverlayWithEquallySizedVariants) IsC() bool {
	return o.Discriminant == OverlayWithEquallySizedVariantsDiscriminantC
}
func (o *OverlayWithEquallySizedVariants) AsC() *OverlayStructVariant {
	if !o.IsC() {
		return nil
	}
	return (*OverlayStructVariant)(unsafe.Pointer(&o.variant))
}

func (o OverlayWithEquallySizedVariants) IsD() bool {
	return o.Discriminant == OverlayWithEquallySizedVariantsDiscriminantD
}
func (o *OverlayWithEquallySizedVariants) AsD() *uint64 {
	if !o.IsD() {
		return nil
	}
	return (*uint64)(unsafe.Pointer(&o.variant))
}

type OverlayWithDifferentlySizedVariantsDiscriminant uint64

const (
	OverlayWithDifferentlySizedVariantsDiscriminantA OverlayWithDifferentlySizedVariantsDiscriminant = 1
	OverlayWithDifferentlySizedVariantsDiscriminantB OverlayWithDifferentlySizedVariantsDiscriminant = 2
	OverlayWithDifferentlySizedVariantsDiscriminantC OverlayWithDifferentlySizedVariantsDiscriminant = 3
)

type OverlayWithDifferentlySizedVariants struct {
	Discriminant OverlayWithDifferentlySizedVariantsDiscriminant
	variant      [8]byte
}

func (o OverlayWithDifferentlySizedVariants) IsA() bool {
	return o.Discriminant == OverlayWithDifferentlySizedVariantsDiscriminantA
}
func (o *OverlayWithDifferentlySizedVariants) AsA() *OverlayStructVariant {
	if !o.IsA() {
		return nil
	}
	return (*OverlayStructVariant)(unsafe.Pointer(&o.variant))
}

func (o OverlayWithDifferentlySizedVariants) IsB() bool {
	return o.Discriminant == OverlayWithDifferentlySizedVariantsDiscriminantB
}
func (o *OverlayWithDifferentlySizedVariants) AsB() *uint32 {
	if !o.IsB() {
		return nil
	}
	return (*uint32)(unsafe.Pointer(&o.variant))
}

func (o OverlayWithDifferentlySizedVariants) IsC() bool {
	return o.Discriminant == OverlayWithDifferentlySizedVariantsDiscriminantC
}
func (o *OverlayWithDifferentlySizedVariants) AsC() *bool {
	if !o.IsC() {
		return nil
	}
	return (*bool)(unsafe.Pointer(&o.variant))
}

type StructWithOverlayMembers struct {
	Overlay1 OverlayWithEquallySizedVariants     `json:"overlay1"`
	Overlay2 OverlayWithDifferentlySizedVariants `json:"overlay2"`
}
