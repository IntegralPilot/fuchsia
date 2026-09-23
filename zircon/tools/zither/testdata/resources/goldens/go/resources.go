// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zither.resources (//zircon/tools/zither/testdata/resources/resources.test.fidl)
// by zither, a Fuchsia platform tool.

package resources

type Subtype uint32

const (
	SubtypeA Subtype = 0
	SubtypeB Subtype = 1
)

// This is a handle.
type Handle uint32

type StructWithHandleMembers struct {
	UntypedHandle Handle `json:"untyped_handle"`
	HandleA       Handle `json:"handle_a"`
	HandleB       Handle `json:"handle_b"`
}
