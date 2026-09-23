// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package coding_tables

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"unicode"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	cpp "go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen_cpp"
	"golang.org/x/exp/slices"
)

type Root struct {
	// Forward declarations of coding tables.
	// Includes both our own and ones linked in from direct dependencies.
	ForwardDecls []forwardDecl

	// Declarations (external linkage)
	Bits    []bits
	Enums   []enum
	Structs []struct_
	Tables  []table
	Unions  []union

	// Types used in declarations (internal linkage)
	Arrays  []array
	Vectors []vector
	Strings []string_
	Handles []handle
	Boxes   []box
}

type forwardDecl struct {
	Name         string
	CType        string
	StorageClass string
}

type bits struct {
	Name           string
	FidlName       fidlgen.EncodedCompoundIdentifier
	UnderlyingType string
	Strictness     string
	Mask           uint64
}

type enum struct {
	Name           string
	FidlName       fidlgen.EncodedCompoundIdentifier
	UnderlyingType string
	Strictness     string
	Validator      string
	Values         []string
}

type struct_ struct {
	Name        string
	FidlName    fidlgen.EncodedCompoundIdentifier
	Emptiness   string
	InlineSize  int
	MembersName string
	Members     []structMember
}

type structMember struct {
	Offset int
	// Set for non-padding members.
	Type         string
	Resourceness string
	// Set for padding members.
	PaddingMaskBitWidth int
	PaddingMask         uint64
}

type table struct {
	Name         string
	FidlName     fidlgen.EncodedCompoundIdentifier
	Resourceness string
	MembersName  string
	Members      []tableMember
}

type tableMember struct {
	Ordinal int
	Type    string
}

type union struct {
	Name         string
	FidlName     fidlgen.EncodedCompoundIdentifier
	NullableName string
	Strictness   string
	Resourceness string
	MembersName  string
	Members      []unionMember
}

type unionMember struct {
	Type string
}

type array struct {
	Name        string
	ElementType string
	InlineSize  int
	ElementSize int
}

type vector struct {
	Name        string
	ElementType string
	MaxCount    uint32
	Nullability string
	ElementSize int
}

type string_ struct {
	Name        string
	MaxCount    uint32
	Nullability string
}

type handle struct {
	Name        string
	ObjectType  string
	Rights      string
	Nullability string
}

type box struct {
	Name       string
	StructName string
}

type compiler struct {
	library   fidlgen.EncodedLibraryIdentifier
	decls     fidlgen.DeclInfoMap
	structs   map[fidlgen.EncodedCompoundIdentifier]*fidlgen.Struct
	seenTypes map[string]struct{}
	out       Root
}

func (c *compiler) compileForwardDecl(name string, typ fidlgen.Type) forwardDecl {
	storage := "static"
	if typ.Kind == fidlgen.IdentifierType {
		info, ok := c.decls[typ.Identifier]
		if !ok {
			panic(fmt.Sprintf("identifier not in decl map: %s", typ.Identifier))
		}
		if info.Type != fidlgen.StructDeclType || !typ.Nullable {
			if typ.Identifier.LibraryName() == c.library {
				storage = ""
			} else {
				// Extern is the default, but we use it to emphasize that this
				// coding table will be linked in from another translation unit.
				storage = "extern"
			}
		}
	}
	return forwardDecl{
		Name:         name,
		CType:        forwardDeclCType(typ, c.decls),
		StorageClass: storage,
	}
}

func (c *compiler) compileDecl(decl fidlgen.Decl) {
	// Skip decls from other libraries, e.g. from "external_structs".
	if decl.GetName().LibraryName() != c.library {
		return
	}
	switch decl := decl.(type) {
	case *fidlgen.Bits:
		c.out.Bits = append(c.out.Bits, c.compileBits(*decl))
	case *fidlgen.Enum:
		c.out.Enums = append(c.out.Enums, c.compileEnum(*decl))
	case *fidlgen.Struct:
		c.out.Structs = append(c.out.Structs, c.compileStruct(*decl))
	case *fidlgen.Table:
		c.out.Tables = append(c.out.Tables, c.compileTable(*decl))
	case *fidlgen.Union:
		c.out.Unions = append(c.out.Unions, c.compileUnion(*decl))
	case *fidlgen.Alias, *fidlgen.Const, *fidlgen.NewType, *fidlgen.Overlay,
		*fidlgen.Protocol, *fidlgen.Resource, *fidlgen.Service:
		// These declarations don't go in coding tables.
	default:
		panic(fmt.Sprintf("unexpected decl type: %T", decl))
	}
}

func (c *compiler) compileBits(v fidlgen.Bits) bits {
	mask, err := strconv.ParseUint(v.Mask, 10, 64)
	if err != nil {
		panic(fmt.Sprintf("cannot parse bits mask: %s", err))
	}
	return bits{
		FidlName:       v.Name,
		Name:           compileDeclName(v.Name),
		UnderlyingType: compilePrimitiveSubtype(v.Type.PrimitiveSubtype),
		Strictness:     compileStrictness(v.Strictness),
		Mask:           mask,
	}
}

func (c *compiler) compileEnum(v fidlgen.Enum) enum {
	var validator string
	if v.IsStrict() {
		validator = "EnumValidatorFor_" + nameDecl(v.Name)
	}
	var values []string
	for _, m := range v.Members {
		// TODO(https://fxbug.dev/42086098): Centralize C++ integer literal logic.
		var expr string
		if m.Value.Value == "-9223372036854775808" {
			expr = "-9223372036854775807 - 1"
		} else if strings.HasPrefix(m.Value.Value, "-") {
			expr = m.Value.Value
		} else {
			expr = m.Value.Value + "u"
		}
		values = append(values, expr)
	}
	return enum{
		FidlName:       v.Name,
		Name:           compileDeclName(v.Name),
		UnderlyingType: compilePrimitiveSubtype(v.Type),
		Strictness:     compileStrictness(v.Strictness),
		Validator:      validator,
		Values:         values,
	}
}

func (c *compiler) compileStruct(v fidlgen.Struct) struct_ {
	var members []structMember
	c.fillStructNonPaddingMembers(v, 0, &members)
	c.fillStructPaddingMembers(v, &members)
	slices.SortFunc(members, func(a, b structMember) int { return a.Offset - b.Offset })
	return struct_{
		FidlName:    v.Name,
		Name:        compileDeclName(v.Name),
		Emptiness:   compileEmptiness(v.Members),
		InlineSize:  v.TypeShapeV2.InlineSize,
		MembersName: compileMembersName(v.Name, len(members)),
		Members:     members,
	}
}

func (c *compiler) fillStructNonPaddingMembers(v fidlgen.Struct, baseOffset int, members *[]structMember) {
	for _, m := range v.Members {
		offset := baseOffset + m.FieldShapeV2.Offset
		if nested, ok := c.structs[m.Type.Identifier]; ok && !m.Type.Nullable {
			c.fillStructNonPaddingMembers(*nested, offset, members)
		} else if c.canCopyWithoutValidation(m.Type) {
			// No coding table member is needed.
		} else {
			*members = append(*members, structMember{
				Offset:       offset,
				Type:         c.compilePointerToType(m.Type),
				Resourceness: compileResourceness(c.decls.LookupResourceness(m.Type)),
			})
		}
	}
}

func (c *compiler) fillStructPaddingMembers(v fidlgen.Struct, members *[]structMember) {
	markers := v.BuildPaddingMarkers(fidlgen.PaddingConfig{
		FlattenStructs: true,
		ResolveStruct:  func(eci fidlgen.EncodedCompoundIdentifier) *fidlgen.Struct { return c.structs[eci] },
	})
	for _, marker := range markers {
		*members = append(*members, structMember{
			Offset:              marker.Offset,
			PaddingMaskBitWidth: marker.MaskBitWidth,
			PaddingMask:         marker.Mask,
		})
	}
}

func (c *compiler) canCopyWithoutValidation(typ fidlgen.Type) bool {
	switch typ.Kind {
	case fidlgen.PrimitiveType:
		return typ.PrimitiveSubtype != fidlgen.Bool
	case fidlgen.ArrayType:
		return c.canCopyWithoutValidation(*typ.ElementType)
	case fidlgen.IdentifierType:
		if s, ok := c.structs[typ.Identifier]; ok && !typ.Nullable && !s.TypeShapeV2.HasPadding {
			for _, m := range s.Members {
				if !c.canCopyWithoutValidation(m.Type) {
					return false
				}
			}
			return true
		}
	}
	return false
}

func (c *compiler) compileTable(v fidlgen.Table) table {
	var members []tableMember
	for _, m := range v.Members {
		members = append(members, tableMember{
			Ordinal: m.Ordinal,
			Type:    c.compilePointerToType(m.Type),
		})
	}
	return table{
		FidlName:     v.Name,
		Name:         compileDeclName(v.Name),
		Resourceness: compileResourceness(v.Resourceness),
		MembersName:  compileMembersName(v.Name, len(members)),
		Members:      members,
	}
}

func (c *compiler) compileUnion(v fidlgen.Union) union {
	nullableType := fidlgen.Type{
		Kind:       fidlgen.IdentifierType,
		Identifier: v.Name,
		Nullable:   true,
	}
	var members []unionMember
	for _, m := range v.Members {
		for len(members) < m.Ordinal-1 {
			members = append(members, unionMember{Type: "NULL"})
		}
		members = append(members, unionMember{
			Type: c.compilePointerToType(m.Type),
		})
	}
	return union{
		FidlName:     v.Name,
		Name:         compileDeclName(v.Name),
		NullableName: compileTypeName(nullableType, c.decls),
		Strictness:   compileStrictness(v.Strictness),
		Resourceness: compileResourceness(v.Resourceness),
		MembersName:  compileMembersName(v.Name, len(v.Members)),
		Members:      members,
	}
}

func (c *compiler) compilePointerToType(typ fidlgen.Type) string {
	return "(const fidl_type_t*)&" + c.compilePointerToTypeWithoutCast(typ)
}

func (c *compiler) compilePointerToTypeWithoutCast(typ fidlgen.Type) string {
	name := compileTypeName(typ, c.decls)
	if _, ok := c.seenTypes[name]; !ok {
		c.seenTypes[name] = struct{}{}
		c.compileType(name, typ)
	}
	return name
}

func (c *compiler) compileType(name string, typ fidlgen.Type) {
	// Skip the forward declaration for types from #include <lib/fidl/internal.h>.
	if !strings.HasPrefix(name, "fidl_internal_k") {
		c.out.ForwardDecls = append(c.out.ForwardDecls, c.compileForwardDecl(name, typ))
	}
	switch typ.Kind {
	case fidlgen.ArrayType:
		c.out.Arrays = append(c.out.Arrays, c.compileArray(name, typ))
	case fidlgen.VectorType:
		c.out.Vectors = append(c.out.Vectors, c.compileVector(name, typ))
	case fidlgen.StringType:
		c.out.Strings = append(c.out.Strings, c.compileString(name, typ))
	case fidlgen.HandleType, fidlgen.EndpointType:
		c.out.Handles = append(c.out.Handles, c.compileHandle(name, typ))
	case fidlgen.IdentifierType:
		info, ok := c.decls[typ.Identifier]
		if !ok {
			panic(fmt.Sprintf("identifier not in decl map: %s", typ.Identifier))
		}
		switch info.Type {
		case fidlgen.StructDeclType:
			if typ.Nullable {
				c.out.Boxes = append(c.out.Boxes, c.compileBox(name, typ))
			}
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType, fidlgen.TableDeclType, fidlgen.UnionDeclType:
			// Do nothing. All we need for these is the forward declaration above.
		default:
			panic(fmt.Sprintf("identifier type refers to unexpected decl type: %s", info.Type))
		}
	case fidlgen.PrimitiveType, fidlgen.InternalType:
		// Do nothing.
	case fidlgen.ZxExperimentalPointerType, fidlgen.StringArray:
		panic(fmt.Sprintf("unsupported type kind for coding tables: %s", typ.Kind))
	default:
		panic(fmt.Sprintf("unexpected type kind: %s", typ.Kind))
	}
}

func (c *compiler) compileArray(name string, typ fidlgen.Type) array {
	return array{
		Name:        name,
		ElementType: c.compilePointerToType(*typ.ElementType),
		InlineSize:  typ.TypeShapeV2.InlineSize,
		ElementSize: typ.ElementType.TypeShapeV2.InlineSize,
	}
}

func (c *compiler) compileVector(name string, typ fidlgen.Type) vector {
	return vector{
		Name:        name,
		ElementType: c.compilePointerToType(*typ.ElementType),
		MaxCount:    compileMaxCount(typ.ElementCount),
		Nullability: compileNullability(typ.Nullable),
		ElementSize: typ.ElementType.TypeShapeV2.InlineSize,
	}
}

func (c *compiler) compileString(name string, typ fidlgen.Type) string_ {
	return string_{
		Name:        name,
		MaxCount:    compileMaxCount(typ.ElementCount),
		Nullability: compileNullability(typ.Nullable),
	}
}

func (c *compiler) compileHandle(name string, typ fidlgen.Type) handle {
	info := cpp.FieldHandleInformation(typ, c.decls)
	return handle{
		Name:        name,
		ObjectType:  info.ObjectType,
		Rights:      info.Rights,
		Nullability: compileNullability(typ.Nullable),
	}
}

func (c *compiler) compileBox(name string, typ fidlgen.Type) box {
	return box{
		Name: name,
		StructName: c.compilePointerToTypeWithoutCast(fidlgen.Type{
			Kind:       fidlgen.IdentifierType,
			Identifier: typ.Identifier,
			Nullable:   false,
		}),
	}
}

func compileDeclName(eci fidlgen.EncodedCompoundIdentifier) string {
	return nameDecl(eci) + "Table"
}

func compileTypeName(typ fidlgen.Type, decls fidlgen.DeclInfoMap) string {
	return nameType(typ, decls) + "Table"
}

func compileMembersName(eci fidlgen.EncodedCompoundIdentifier, numMembers int) string {
	if numMembers == 0 {
		return "NULL"
	}
	return "Fields" + lengthPrefixed(nameDecl(eci))
}

func compileStrictness(s fidlgen.Strictness) string {
	switch s {
	case fidlgen.IsStrict:
		return "kFidlStrictness_Strict"
	case fidlgen.IsFlexible:
		return "kFidlStrictness_Flexible"
	default:
		panic("unexpected strictness")
	}
}

func compileResourceness(r fidlgen.Resourceness) string {
	switch fidlgen.Resourceness(r) {
	case fidlgen.IsValueType:
		return "kFidlIsResource_NotResource"
	case fidlgen.IsResourceType:
		return "kFidlIsResource_Resource"
	default:
		panic("unexpected resourceness")
	}
}

func compileNullability(nullable bool) string {
	if nullable {
		return "kFidlNullability_Nullable"
	}
	return "kFidlNullability_Nonnullable"
}

func compilePrimitiveSubtype(p fidlgen.PrimitiveSubtype) string {
	return "kFidlCodedPrimitiveSubtype_" + namePrimitiveSubtype(p)
}

func compileEmptiness(members []fidlgen.StructMember) string {
	if len(members) == 0 {
		return "kFidlEmpty_IsEmpty"
	}
	return "kFidlEmpty_IsNotEmpty"
}

func compileMaxCount(n *int) uint32 {
	if n == nil {
		return math.MaxUint32
	}
	return uint32(*n)
}

func forwardDeclCType(typ fidlgen.Type, decls fidlgen.DeclInfoMap) string {
	switch typ.Kind {
	case fidlgen.ArrayType:
		return "FidlCodedArray"
	case fidlgen.VectorType:
		return "FidlCodedVector"
	case fidlgen.StringType:
		return "FidlCodedString"
	case fidlgen.HandleType, fidlgen.EndpointType:
		return "FidlCodedHandle"
	case fidlgen.IdentifierType:
		info, ok := decls[typ.Identifier]
		if !ok {
			panic(fmt.Sprintf("identifier not in decl map: %s", typ.Identifier))
		}
		switch info.Type {
		case fidlgen.BitsDeclType:
			return "FidlCodedBits"
		case fidlgen.EnumDeclType:
			return "FidlCodedEnum"
		case fidlgen.StructDeclType:
			if typ.Nullable {
				return "FidlCodedStructPointer"
			}
			return "FidlCodedStruct"
		case fidlgen.TableDeclType:
			return "FidlCodedTable"
		case fidlgen.UnionDeclType:
			return "FidlCodedUnion"
		default:
			panic(fmt.Sprintf("identifier type refers to unexpected decl type: %s", info.Type))
		}
	case fidlgen.PrimitiveType, fidlgen.InternalType:
		panic(fmt.Sprintf("should not need to forward declare type kind: %s", typ.Kind))
	case fidlgen.ZxExperimentalPointerType, fidlgen.StringArray:
		panic(fmt.Sprintf("unsupported type kind for coding tables: %s", typ.Kind))
	default:
		panic(fmt.Sprintf("unexpected type kind: %s", typ.Kind))
	}
}

func nameType(typ fidlgen.Type, decls fidlgen.DeclInfoMap) string {
	switch typ.Kind {
	case fidlgen.ArrayType:
		return fmt.Sprintf("Array%s_%s", nameSize(typ.ElementCount), lengthPrefixed(nameType(*typ.ElementType, decls)))
	case fidlgen.VectorType:
		return fmt.Sprintf("Vector%s%s_%s", nameSize(typ.ElementCount), nameNullability(typ.Nullable), lengthPrefixed(nameType(*typ.ElementType, decls)))
	case fidlgen.StringType:
		return fmt.Sprintf("String%s%s", nameSize(typ.ElementCount), nameNullability(typ.Nullable))
	case fidlgen.HandleType:
		return fmt.Sprintf("Handle%s%d%s", typ.HandleSubtype, typ.HandleRights, nameNullability(typ.Nullable))
	case fidlgen.EndpointType:
		switch typ.Role {
		case fidlgen.ClientRole:
			return fmt.Sprintf("Protocol%s%s", lengthPrefixed(nameDecl(typ.Protocol)), nameNullability(typ.Nullable))
		case fidlgen.ServerRole:
			// TODO(https://fxbug.dev/42149402): This reimplements a bug from a
			// previous implementation - the name of the protocol isn't actually
			// included in the `Request` type. Fix this separately.
			return fmt.Sprintf("Request0%s", nameNullability(typ.Nullable))
		default:
			panic(fmt.Sprintf("unexpected fidlgen.EndpointRole: %#v", typ.Role))
		}
	case fidlgen.PrimitiveType:
		return fmt.Sprintf("fidl_internal_k%s", namePrimitiveSubtype(typ.PrimitiveSubtype))
	case fidlgen.InternalType:
		return fmt.Sprintf("fidl_internal_k%s", nameInternalSubtype(typ.InternalSubtype))
	case fidlgen.IdentifierType:
		info, ok := decls[typ.Identifier]
		if !ok {
			panic(fmt.Sprintf("identifier not in decl map: %s", typ.Identifier))
		}
		if typ.Identifier.LibraryName() == "zx" {
			switch typ.Identifier {
			// We emit zx_obj_type_t and zx_rights_t for these types in
			// bindings, so treat them as such in coding tables as well.
			case "zx/ObjType", "zx/Rights":
				return nameType(fidlgen.Type{Kind: fidlgen.PrimitiveType, PrimitiveSubtype: fidlgen.Uint32}, decls)
			case "zx/InstantMono", "zx/InstantBoot", "zx/InstantMonoTicks", "zx/InstantBootTicks":
				return nameType(fidlgen.Type{Kind: fidlgen.PrimitiveType, PrimitiveSubtype: fidlgen.Int64}, decls)
			default:
				panic(fmt.Sprintf("unexpected zx type: %s", typ.Identifier))
			}
		}
		switch info.Type {
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType, fidlgen.TableDeclType:
			return nameDecl(typ.Identifier)
		case fidlgen.StructDeclType:
			if typ.Nullable {
				return fmt.Sprintf("Pointer%s", lengthPrefixed(nameDecl(typ.Identifier)))
			}
			return nameDecl(typ.Identifier)
		case fidlgen.UnionDeclType:
			if typ.Nullable {
				return fmt.Sprintf("%sNullableRef", nameDecl(typ.Identifier))
			}
			return nameDecl(typ.Identifier)
		default:
			panic(fmt.Sprintf("identifier type refers to unexpected decl type: %s", info.Type))
		}
	case fidlgen.ZxExperimentalPointerType, fidlgen.StringArray:
		panic(fmt.Sprintf("unsupported type kind for coding tables: %s", typ.Kind))
	default:
		panic(fmt.Sprintf("unexpected type kind: %s", typ.Kind))
	}
}

func nameDecl(eci fidlgen.EncodedCompoundIdentifier) string {
	s := string(eci)
	s = strings.Replace(s, "/", "_", 1)
	s = strings.ReplaceAll(s, ".", "_")
	return s
}

func nameSize(n *int) string {
	if n == nil {
		return "unbounded"
	}
	return strconv.Itoa(*n)
}

func nameNullability(nullable bool) string {
	if nullable {
		return "nullable"
	}
	return "notnullable"
}

func namePrimitiveSubtype(t fidlgen.PrimitiveSubtype) string {
	s := string(t)
	return string(unicode.ToUpper(rune(s[0]))) + s[1:]
}

func nameInternalSubtype(t fidlgen.InternalSubtype) string {
	switch t {
	case fidlgen.FrameworkErr:
		return "FrameworkErr"
	default:
		panic(fmt.Sprintf("unexpected internal subtype: %s", t))
	}
}

func lengthPrefixed(s string) string {
	return fmt.Sprintf("%d%s", len(s), s)
}

func Compile(r fidlgen.Root) Root {
	c := compiler{
		library:   r.Name,
		decls:     r.DeclInfo(),
		structs:   make(map[fidlgen.EncodedCompoundIdentifier]*fidlgen.Struct),
		seenTypes: make(map[string]struct{}),
	}
	for i := range r.Structs {
		s := &r.Structs[i]
		c.structs[s.Name] = s
	}
	r.ForEachDecl(c.compileDecl)
	return c.out
}
