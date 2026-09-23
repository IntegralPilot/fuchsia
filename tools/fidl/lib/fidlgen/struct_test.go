// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestBuildPaddingMarkersWithoutFlattening(t *testing.T) {
	type testCase struct {
		name string
		in   Struct
		out  []PaddingMarker
	}
	testCases := []testCase{
		{
			name: "empty",
			in:   Struct{},
			out:  nil,
		},
		{
			name: "no padding 8-bytes",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 8,
					Alignment:  8,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  4,
							Padding: 0,
						},
					},
				},
			},
			out: nil,
		},
		{
			name: "no padding 4-bytes",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 4,
					Alignment:  4,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  2,
							Padding: 0,
						},
					},
				},
			},
			out: nil,
		},
		{
			name: "no padding 2-bytes",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 2,
					Alignment:  2,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  1,
							Padding: 0,
						},
					},
				},
			},
			out: nil,
		},
		{
			name: "no padding 1-byte",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 1,
					Alignment:  1,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
				},
			},
			out: nil,
		},
		{
			name: "8-byte struct with 2 bytes of padding at end",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 8,
					Alignment:  8,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  4,
							Padding: 2,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       0,
					MaskBitWidth: 64,
					Mask:         0xff_ff_00_00_00_00_00_00,
				},
			},
		},
		{
			name: "4-byte struct with 1 byte of padding at end",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 4,
					Alignment:  4,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  2,
							Padding: 1,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       0,
					MaskBitWidth: 32,
					Mask:         0xff_00_00_00,
				},
			},
		},
		{
			name: "2-byte struct with 1 byte padding at end",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 2,
					Alignment:  2,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 1,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       0,
					MaskBitWidth: 16,
					Mask:         0xff_00,
				},
			},
		},
		{
			name: "padding at end of 8-byte chunk, before next chunk",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 16,
					Alignment:  8,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  4,
							Padding: 2,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  8,
							Padding: 0,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       0,
					MaskBitWidth: 64,
					Mask:         0xff_ff_00_00_00_00_00_00,
				},
			},
		},
		{
			name: "padding in middle of 4-byte block",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 4,
					Alignment:  4,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 1,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  2,
							Padding: 0,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       0,
					MaskBitWidth: 32,
					Mask:         0x00_00_ff_00,
				},
			},
		},
		{
			name: "8 byte mask with non-zero offset",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 16,
					Alignment:  8,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  8,
							Padding: 4,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       8,
					MaskBitWidth: 64,
					Mask:         0xff_ff_ff_ff_00_00_00_00,
				},
			},
		},
		{
			name: "4 byte mask with non-zero offset",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 12,
					Alignment:  4,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  8,
							Padding: 2,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       8,
					MaskBitWidth: 32,
					Mask:         0xff_ff_00_00,
				},
			},
		},
		{
			name: "2 byte mask with non-zero offset",
			in: Struct{
				TypeShapeV2: TypeShape{
					InlineSize: 10,
					Alignment:  2,
				},
				Members: []StructMember{
					{
						FieldShapeV2: FieldShape{
							Offset:  0,
							Padding: 0,
						},
					},
					{
						FieldShapeV2: FieldShape{
							Offset:  8,
							Padding: 1,
						},
					},
				},
			},
			out: []PaddingMarker{
				{
					Offset:       8,
					MaskBitWidth: 16,
					Mask:         0xff_00,
				},
			},
		},
	}
	for _, testCase := range testCases {
		out := testCase.in.BuildPaddingMarkers(PaddingConfig{})
		if diff := cmp.Diff(testCase.out, out); diff != "" {
			t.Errorf("%s:\nexpected != actual (-want +got)\n%s", testCase.name, diff)
		}
	}
}

func TestBuildPaddingMarkersFlatteningStruct(t *testing.T) {
	var innerStructIdentifier EncodedCompoundIdentifier = "abcd"
	innerStruct := Struct{
		TypeShapeV2: TypeShape{
			InlineSize: 4,
			Alignment:  4,
		},
		Members: []StructMember{
			{
				FieldShapeV2: FieldShape{
					Offset:  0,
					Padding: 3,
				},
			},
		},
	}
	input := Struct{
		TypeShapeV2: TypeShape{
			InlineSize: 8,
			Alignment:  4,
		},
		Members: []StructMember{
			{
				FieldShapeV2: FieldShape{
					Offset:  0,
					Padding: 4,
				},
				Type: Type{
					Kind:       IdentifierType,
					Identifier: innerStructIdentifier,
				},
			},
		},
	}
	out := input.BuildPaddingMarkers(PaddingConfig{
		FlattenStructs: true,
		ResolveStruct: func(identifier EncodedCompoundIdentifier) *Struct {
			if identifier == innerStructIdentifier {
				return &innerStruct
			}
			return nil
		},
	})
	expected := []PaddingMarker{
		{
			Offset:       0,
			MaskBitWidth: 32,
			Mask:         0xff_ff_ff_00,
		},
		{
			Offset:       4,
			MaskBitWidth: 32,
			Mask:         0xff_ff_ff_ff,
		},
	}
	if diff := cmp.Diff(expected, out); diff != "" {
		t.Errorf("expected != actual (-want +got)\n%s", diff)
	}
}

func TestBuildPaddingMarkersFlatteningArrayOfStruct(t *testing.T) {
	var innerStructIdentifier EncodedCompoundIdentifier = "abcd"
	innerStruct := Struct{
		TypeShapeV2: TypeShape{
			InlineSize: 4,
			Alignment:  4,
		},
		Members: []StructMember{
			{
				FieldShapeV2: FieldShape{
					Offset:  0,
					Padding: 3,
				},
			},
		},
	}
	count := 3
	input := Struct{
		TypeShapeV2: TypeShape{
			InlineSize: 12,
			Alignment:  4,
		},
		Members: []StructMember{
			{
				FieldShapeV2: FieldShape{
					Offset:  0,
					Padding: 0,
				},
				Type: Type{
					Kind:         ArrayType,
					ElementCount: &count,
					ElementType: &Type{
						Kind:       IdentifierType,
						Identifier: innerStructIdentifier,
					},
				},
			},
		},
	}
	out := input.BuildPaddingMarkers(PaddingConfig{
		FlattenStructs: true,
		FlattenArrays:  true,
		ResolveStruct: func(identifier EncodedCompoundIdentifier) *Struct {
			if identifier == innerStructIdentifier {
				return &innerStruct
			}
			return nil
		},
	})
	expected := []PaddingMarker{
		{
			Offset:       0,
			MaskBitWidth: 32,
			Mask:         0xff_ff_ff_00,
		},
		{
			Offset:       4,
			MaskBitWidth: 32,
			Mask:         0xff_ff_ff_00,
		},
		{
			Offset:       8,
			MaskBitWidth: 32,
			Mask:         0xff_ff_ff_00,
		},
	}
	if diff := cmp.Diff(expected, out); diff != "" {
		t.Errorf("expected != actual (-want +got)\n%s", diff)
	}
}
