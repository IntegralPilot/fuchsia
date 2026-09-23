// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type TableFrameItem *TableMember

// These correspond to templated classes forward-declared in
// //sdk/lib/fidl/cpp/wire/include/lib/fidl/cpp/wire/wire_types.h
var (
	WireTableFrame           = fidlNs.member("WireTableFrame")
	WireTableBuilder         = fidlNs.member("WireTableBuilder")
	WireTableExternalBuilder = fidlNs.member("WireTableExternalBuilder")
)

// TableName stores all of the information necessary to use a table as a
// payload. Unlike structs, tables always use a single "payload" argument
// pointing to the underlying table/table type, so for externally defined tables
// we only need to store the name and (optional) owning result type of the
// table, rather than the entire, flattenable declaration with all of its
// members.
type TableName struct {
	nameVariants
}

func (*TableName) Kind() declKind {
	return Kinds.Table
}

// AsParameters renders the referenced table as a parameter list of length 1.
func (u *TableName) AsParameters(ty *Type, hi *HandleInformation) []Parameter {
	return []Parameter{{
		Type:              *ty,
		nameVariants:      ty.nameVariants,
		OffsetV2:          0,
		HandleInformation: hi,
	}}
}

var _ Kinded = (*TableName)(nil)
var _ Payloader = (*TableName)(nil)
var _ namespaced = (*TableName)(nil)

type Table struct {
	TableName
	Attributes
	fidlgen.Resourceness
	AnonymousChildren   []ScopedLayout
	CodingTableType     name
	Members             []TableMember
	BiggestOrdinal      int
	BackingBufferTypeV2 string
	TypeShapeV2         TypeShape

	// WireTableFrame is the name of the table frame type associated with
	// this table in wire domain objects.
	WireTableFrame name
	// TODO(https://fxbug.dev/42069447): document these
	WireTableBuilder         name
	WireTableExternalBuilder name
	// FrameItems stores the members in ordinal order; "null" for reserved.
	FrameItems   []TableFrameItem
	Serializable fidlgen.Serializable
}

var _ Kinded = (*Table)(nil)
var _ Payloader = (*Table)(nil)
var _ namespaced = (*Table)(nil)

type TableMember struct {
	Attributes
	nameVariants
	Type               Type
	Ordinal            int
	FieldPresenceIsSet string
	FieldPresenceSet   string
	FieldPresenceClear string
	FieldDataName      string
	MethodHasName      string
	MethodClearName    string
	ValueUnionName     string
	HandleInformation  *HandleInformation
	NaturalConstraint  string
	WireConstraint     string
}

func (tm TableMember) NameAndType() (string, Type) {
	return tm.Name(), tm.Type
}

func (tm TableMember) StorageName() string {
	return tm.Name() + "_"
}

func (c *compiler) compileTableMember(val fidlgen.TableMember, index int) TableMember {
	t := c.compileType(val.Type, val.MaybeFromAlias)

	return TableMember{
		Attributes:         Attributes{val.Attributes},
		nameVariants:       tableMemberContext.transform(val.Name),
		Type:               t,
		Ordinal:            val.Ordinal,
		FieldPresenceIsSet: fmt.Sprintf("field_presence_.IsSet<%d>()", val.Ordinal-1),
		FieldPresenceSet:   fmt.Sprintf("field_presence_.Set<%d>()", val.Ordinal-1),
		FieldPresenceClear: fmt.Sprintf("field_presence_.Clear<%d>()", val.Ordinal-1),
		FieldDataName:      fmt.Sprintf("%s_value_", val.Name),
		MethodHasName:      fmt.Sprintf("has_%s", val.Name),
		MethodClearName:    fmt.Sprintf("clear_%s", val.Name),
		ValueUnionName:     fmt.Sprintf("ValueUnion_%s", val.Name),
		HandleInformation:  c.fieldHandleInformation(val.Type),
		NaturalConstraint:  t.NaturalFieldConstraint,
		WireConstraint:     t.WireFieldConstraint,
	}
}

func (c *compiler) compileTable(val fidlgen.Table) *Table {
	name := c.compileNameVariants(val.Name)
	codingTableType := name.Wire.ns.member(c.compileCodingTableType(val.Name))
	r := Table{
		TableName:         TableName{nameVariants: name},
		Attributes:        Attributes{val.Attributes},
		AnonymousChildren: c.getAnonymousChildren(val),
		TypeShapeV2:       TypeShape{val.TypeShapeV2},
		Resourceness:      val.Resourceness,
		CodingTableType:   codingTableType,
		Members:           nil,
		BiggestOrdinal:    0,
		BackingBufferTypeV2: computeAllocation(
			TypeShape{val.TypeShapeV2}.MaxTotalSize(), TypeShape{val.TypeShapeV2}.MaxHandles, boundednessBounded).
			BackingBufferType(),
		WireTableFrame:           WireTableFrame.template(name.Wire),
		WireTableBuilder:         WireTableBuilder.template(name.Wire),
		WireTableExternalBuilder: WireTableExternalBuilder.template(name.Wire),
		Serializable:             val.GetSerializable(),
	}

	for i, v := range val.Members {
		m := c.compileTableMember(v, i)
		if m.Ordinal > r.BiggestOrdinal {
			r.BiggestOrdinal = m.Ordinal
		}
		r.Members = append(r.Members, m)
	}

	r.FrameItems = make([]TableFrameItem, r.BiggestOrdinal)
	for index, member := range r.Members {
		r.FrameItems[member.Ordinal-1] = &r.Members[index]
	}

	return &r
}
