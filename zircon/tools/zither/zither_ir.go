// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package zither contains abstractions and utilities shared by the various backends.
package zither

import (
	"fmt"
	"math/bits"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen_cpp"
)

// DeclOrder represents the ordering policy for declarations as they are
// provided to the backend.
type DeclOrder int

const (
	// SourceDeclOrder orders declarations as they were in source:
	// lexicographically by filename across FIDL files and then by location
	// within files.
	SourceDeclOrder DeclOrder = iota

	// DependencyDeclOrder gives a topological sorting of declarations by
	// dependency, so that a declaration is always preceded by the declarations
	// figuring into its definition. Dependency alone, however, gives a partial
	// order; this order is fixed in that it further orders declarations with
	// no interdependencies by source (as above).
	//
	// An order topologically sorted on dependency is a general requirement for
	// any backend generating C-family code. While technically speaking
	// forward-declarations could be used to rearrange the definitions of a
	// number of things, this is not an option with nested struct or union
	// definitions, as the nested layout types would need to be 'complete' in
	// that scope. Accordingly, it is simpler just to deal in full topological
	// sorts.
	DependencyDeclOrder
)

// Element represents a summarized FIDL element (i.e., a declaration or one of
// its members).
type Element interface {
	GetComments() []string
}

var _ = []Element{
	(*Alias)(nil),
	(*Bits)(nil),
	(*BitsMember)(nil),
	(*Const)(nil),
	(*Enum)(nil),
	(*EnumMember)(nil),
	(*Handle)(nil),
	(*OverlayVariant)(nil),
	(*Struct)(nil),
	(*StructMember)(nil),
	(*Syscall)(nil),
	(*SyscallFamily)(nil),
	(*SyscallParameter)(nil),
}

// Decl represents a summarized FIDL declaration.
type Decl interface {
	Element
	GetName() fidlgen.Name
}

var _ = []Decl{
	(*Alias)(nil),
	(*Const)(nil),
	(*Enum)(nil),
	(*Handle)(nil),
	(*Overlay)(nil),
	(*Struct)(nil),
	(*SyscallFamily)(nil),
}

type decl struct {
	Comments []string
	Name     fidlgen.Name
}

func (d decl) GetComments() []string {
	return d.Comments
}

func (d decl) GetName() fidlgen.Name {
	return d.Name
}

func newDecl(d fidlgen.Decl) decl {
	attrs := d.GetAttributes()
	var comments []string
	if !attrs.HasAttribute("no_doc") {
		comments = attrs.DocComments()
	}
	return decl{
		Name:     fidlgen.MustReadName(string(d.GetName())),
		Comments: comments,
	}
}

// Member represents a summarized member of a FIDL layout declaration.
type Member interface {
	Element
	GetName() string
}

var _ = []Member{
	(*BitsMember)(nil),
	(*EnumMember)(nil),
	(*OverlayVariant)(nil),
	(*StructMember)(nil),
	(*Syscall)(nil),
	(*SyscallParameter)(nil),
}

type member struct {
	Comments []string
	Name     string
}

func (m member) GetComments() []string {
	return m.Comments
}

func (m member) GetName() string {
	return m.Name
}

func newMember(m fidlgen.Member) member {
	attrs := m.GetAttributes()
	var comments []string
	if !attrs.HasAttribute("no_doc") {
		comments = attrs.DocComments()
	}
	return member{
		Name:     string(m.GetName()),
		Comments: comments,
	}
}

// DeclWrapper represents an abstract (summarized) FIDL declaration, meant for
// use in template logic and featuring thin wrappers around type assertions for
// deriving concrete types. In normal go code, we would do the type assertions
// directly, but no can do in templates.
type DeclWrapper struct {
	value any
}

func (decl DeclWrapper) Name() fidlgen.Name {
	return decl.AsDecl().GetName()
}

func (decl DeclWrapper) AsDecl() Decl {
	return decl.value.(Decl)
}

func (decl DeclWrapper) IsConst() bool {
	_, ok := decl.value.(*Const)
	return ok
}

func (decl DeclWrapper) AsConst() Const {
	return *decl.value.(*Const)
}

func (decl DeclWrapper) IsEnum() bool {
	_, ok := decl.value.(*Enum)
	return ok
}

func (decl DeclWrapper) AsEnum() Enum {
	return *decl.value.(*Enum)
}

func (decl DeclWrapper) IsBits() bool {
	_, ok := decl.value.(*Bits)
	return ok
}

func (decl DeclWrapper) AsBits() Bits {
	return *decl.value.(*Bits)
}

func (decl DeclWrapper) IsStruct() bool {
	_, ok := decl.value.(*Struct)
	return ok
}

func (decl DeclWrapper) AsStruct() Struct {
	return *decl.value.(*Struct)
}

func (decl DeclWrapper) IsOverlay() bool {
	_, ok := decl.value.(*Overlay)
	return ok
}

func (decl DeclWrapper) AsOverlay() Overlay {
	return *decl.value.(*Overlay)
}

func (decl DeclWrapper) IsAlias() bool {
	_, ok := decl.value.(*Alias)
	return ok
}

func (decl DeclWrapper) IsHandle() bool {
	_, ok := decl.value.(*Handle)
	return ok
}

func (decl DeclWrapper) IsSyscallFamily() bool {
	_, ok := decl.value.(*SyscallFamily)
	return ok
}

func (decl DeclWrapper) AsAlias() Alias {
	return *decl.value.(*Alias)
}

func (decl DeclWrapper) AsHandle() Handle {
	return *decl.value.(*Handle)
}

func (decl DeclWrapper) AsSyscallFamily() SyscallFamily {
	return *decl.value.(*SyscallFamily)
}

// FileSummary is a summarized representation of a FIDL source file.
type FileSummary struct {
	// Library is the associated FIDL library.
	Library fidlgen.LibraryName

	// Source gives the associated source file.
	Source string

	// The contained declarations.
	Decls []DeclWrapper

	// See Deps().
	deps map[string]struct{}

	// See TypeKinds().
	typeKinds map[TypeKind]struct{}
}

// Name is the extension-less basename of the file.
func (summary FileSummary) Name() string {
	name := filepath.Base(summary.Source)
	name = strings.TrimSuffix(name, ".test.fidl")
	name = strings.TrimSuffix(name, ".fidl")
	name = strings.ReplaceAll(name, ".", "_")
	return name
}

// Deps records the (extension-less) file names that this file depends on; we
// say a file "depends" on another if the former has a declaration that depends
// on a declaration in the latter.
func (summary FileSummary) Deps() []string {
	var deps []string
	for dep := range summary.deps {
		deps = append(deps, dep)
	}
	// Sort to account for map access nondeterminism.
	sort.Strings(deps)
	return deps
}

// ReferencedDecls returns full FIDL declaration names referenced by types in
// this file's declarations.
func (summary FileSummary) ReferencedDecls() []string {
	var refs []string
	var visitType func(desc TypeDescriptor)
	visitType = func(desc TypeDescriptor) {
		switch desc.Kind {
		case TypeKindEnum, TypeKindBits, TypeKindStruct, TypeKindOverlay, TypeKindAlias:
			refs = append(refs, desc.Type)
		case TypeKindArray, TypeKindPointer, TypeKindVoidPointer:
			if desc.ElementType != nil {
				visitType(*desc.ElementType)
			}
		}
	}
	for _, decl := range summary.Decls {
		switch d := decl.AsDecl().(type) {
		case *Struct:
			for _, m := range d.Members {
				visitType(m.Type)
			}
		case *Overlay:
			for _, v := range d.Variants {
				visitType(v.Type)
			}
		case *Alias:
			visitType(d.Value)
		case *Const:
			if d.Kind == TypeKindEnum || d.Kind == TypeKindBits {
				refs = append(refs, d.Type)
			}
		}
	}
	return refs
}

// TypeKinds gives the kinds of types contained in this file's declarations.
// This is useful for knowing the precise set of imports to make in the code
// generated from this file.
func (summary FileSummary) TypeKinds() []TypeKind {
	var kinds []TypeKind
	for kind := range summary.typeKinds {
		kinds = append(kinds, kind)
	}
	// Sort to account for map access nondeterminism.
	sort.Slice(kinds, func(i, j int) bool {
		return strings.Compare(string(kinds[i]), string(kinds[j])) < 0
	})
	return kinds
}

type LibrarySummary struct {
	// Library is the associated FIDL library.
	Library fidlgen.LibraryName

	// Documentation gives newline-separated library-level documentation.
	Documentation []string

	Files []FileSummary
}

type declMap map[string]Decl
type memberMap map[string]Member

// Summarize creates FIDL file summaries from FIDL IR. Within each file
// summary, declarations are ordered according to `order`. Further, a callback
// may be provided for extra bookkeeping, which will be called on each
// zither-summarized Decl in topological 'dependency' order (à la
// DependencyDeclOrder), irrespective of the provided `order`.
func Summarize(ir fidlgen.Root, sourceDir string, order DeclOrder, cb func(Decl)) (*LibrarySummary, error) {
	libName, err := fidlgen.ReadLibraryName(string(ir.Name))
	if err != nil {
		return nil, err
	}

	// We will process declarations in topological order (with respect to
	// dependency) and record declarations as we go; that way, we when we
	// process a particular declaration we will have full knowledge of is
	// dependencies, and by extension itself. This ordering exists just for
	// ease of processing and is independent of that prescribed by `order`.
	g := fidlgen_cpp.NewDeclDepGraph(ir)
	fidlDecls := g.SortedDecls()

	locations := make(map[string]fidlgen.Location)
	processedDecls := make(declMap)
	processedConstMembers := make(memberMap)

	filesByName := make(map[string]*FileSummary)
	getFile := func(location fidlgen.Location) *FileSummary {
		src, err := filepath.Rel(sourceDir, location.Filename)
		if err != nil {
			panic(fmt.Sprintf("could not relativize %s against source directory %s: %s", location.Filename, sourceDir, err))
		}

		file, ok := filesByName[location.Filename]
		if !ok {
			file = &FileSummary{
				Library:   libName,
				Source:    src,
				deps:      make(map[string]struct{}),
				typeKinds: make(map[TypeKind]struct{}),
			}
			filesByName[location.Filename] = file
		}
		return file
	}

	for _, fidlDecl := range fidlDecls {
		typeKinds := make(map[TypeKind]struct{})
		var decl Decl
		var constMembers []Member
		var err error
		switch fidlDecl := fidlDecl.(type) {
		case *fidlgen.Const:
			decl, err = newConst(*fidlDecl, processedDecls, processedConstMembers)
			if err == nil {
				typeKinds[decl.(*Const).Kind] = struct{}{}
			}
		case *fidlgen.Enum:
			decl, err = newEnum(*fidlDecl)
			if err == nil {
				for _, m := range decl.(*Enum).Members {
					constMembers = append(constMembers, Member(m))
				}
				typeKinds[TypeKindInteger] = struct{}{}
			}
		case *fidlgen.Bits:
			decl, err = newBits(*fidlDecl)
			if err == nil {
				for _, m := range decl.(*Bits).Members {
					constMembers = append(constMembers, Member(m))
				}
				typeKinds[TypeKindInteger] = struct{}{}
			}
		case *fidlgen.Struct:
			decl, err = newStruct(*fidlDecl, processedDecls, typeKinds)
			if err == nil {
				typeKinds[TypeKindStruct] = struct{}{}
			}
		case *fidlgen.Overlay:
			decl, err = newOverlay(*fidlDecl, processedDecls, typeKinds)
			if err == nil {
				typeKinds[TypeKindOverlay] = struct{}{}
			}
		case *fidlgen.Alias:
			decl, err = newAlias(*fidlDecl, processedDecls, typeKinds)
			if err == nil {
				typeKinds[TypeKindAlias] = struct{}{}
			}
		case *fidlgen.Resource:
			decl, err = newHandle(*fidlDecl, typeKinds)
			if err == nil {
				typeKinds[TypeKindHandle] = struct{}{}
			}
		case *fidlgen.Protocol:
			decl, err = newSyscallFamily(*fidlDecl, processedDecls)
		}
		if err != nil {
			return nil, err
		}

		// A nil `decl` means that it corresponds to an unknown FIDL
		// declaration. Moreover, if `decl` wraps a nil (which does not imply
		// that it is nil itself), then that means that it is defined in terms
		// of unknown elements. In either case, we cannot make sense of the
		// definition and so we skip it.
		//
		// TODO(https://fxbug.dev/42057859): We do not want to silently ignore things
		// that a user might expect to generate results.
		if decl == nil || reflect.ValueOf(decl).IsNil() {
			// Before skipping, we should still record the file that the
			// unrecognized declaration came from (e.g., for determinism in
			// the case where we wish the have the generated set of files
			// match the incoming source file names); calling getFile() has
			// this side-effect.
			getFile(fidlDecl.GetLocation())
			continue
		}

		file := getFile(fidlDecl.GetLocation())
		file.Decls = append(file.Decls, DeclWrapper{decl})
		for kind := range typeKinds {
			file.typeKinds[kind] = struct{}{}
		}

		// Now go back and record the dependents' dependency on this declaration.
		dependents, ok := g.GetDirectDependents(fidlDecl.GetName())
		if !ok {
			panic(fmt.Sprintf("%s not found in declaration graph", decl.GetName()))
		}
		for _, dependent := range dependents {
			dependentFile := getFile(dependent.GetLocation())
			if dependentFile.Source != file.Source {
				dependentFile.deps[file.Name()] = struct{}{}
			}
		}

		declName := decl.GetName().String()
		locations[declName] = fidlDecl.GetLocation()
		processedDecls[declName] = decl
		for _, m := range constMembers {
			processedConstMembers[declName+"."+m.GetName()] = m
		}

		cb(decl)
	}

	var files []FileSummary
	for _, file := range filesByName {
		// Now reorder declarations in the order expected by the backends.
		switch order {
		case SourceDeclOrder:
			sort.Slice(file.Decls, func(i, j int) bool {
				locI := locations[file.Decls[i].Name().String()]
				locJ := locations[file.Decls[j].Name().String()]
				return fidlgen.LocationCmp(locI, locJ)
			})
		case DependencyDeclOrder:
			// Already in this order.
		default:
			panic(fmt.Sprintf("unknown declaration order: %v", order))
		}

		files = append(files, *file)
	}
	return &LibrarySummary{
		Library:       libName,
		Documentation: ir.Attributes.DocComments(),
		Files:         files,
	}, nil
}

// TypeKind gives a rough categorization of FIDL primitive and declaration types.
type TypeKind string

const (
	TypeKindBool        TypeKind = "bool"
	TypeKindInteger     TypeKind = "integer"
	TypeKindSize        TypeKind = "size"
	TypeKindString      TypeKind = "string"
	TypeKindEnum        TypeKind = "enum"
	TypeKindBits        TypeKind = "bits"
	TypeKindArray       TypeKind = "array"
	TypeKindStringArray TypeKind = "string_array"
	TypeKindStruct      TypeKind = "struct"
	TypeKindOverlay     TypeKind = "overlay"
	TypeKindAlias       TypeKind = "alias" // Not a type per se, but conveniently regarded as such.

	// TODO(https://fxbug.dev/42061412): These kinds exist only for the sake of the
	// interim, v2 form of FIDL library zx.
	TypeKindPointer      TypeKind = "pointer"
	TypeKindVoidPointer  TypeKind = "voidptr"
	TypeKindVector       TypeKind = "vector"        // pointer + size_t
	TypeKindVector32     TypeKind = "vector32"      // pointer + uint32_t
	TypeKindVoidVector   TypeKind = "void_vector"   // void* + size_t
	TypeKindVoidVector32 TypeKind = "void_vector32" // void* + uint32_t
	TypeKindHandle       TypeKind = "handle"
)

func (kind TypeKind) IsPointerLike() bool {
	return kind == TypeKindPointer || kind == TypeKindVoidPointer
}

func (kind TypeKind) IsVectorLike() bool {
	return kind == TypeKindVector || kind == TypeKindVector32 || kind == TypeKindVoidVector || kind == TypeKindVoidVector32
}

func (kind TypeKind) IsVector32Like() bool {
	return kind == TypeKindVector32 || kind == TypeKindVoidVector32
}

func (kind TypeKind) IsVoidVectorLike() bool {
	return kind == TypeKindVoidVector || kind == TypeKindVoidVector32
}

// Const is a representation of a constant FIDL declaration.
type Const struct {
	decl

	// Kind is the kind of the constant's type.
	Kind TypeKind

	// Type is the FIDL type of the constant. If Kind is TypeKindEnum or
	// TypeKindBits, then this field encodes a full declaration name of
	// the associated enum or bits.
	Type string

	// Value is the constant's value in string form.
	Value string

	// Element gives whether this constant was defined in terms of another FIDL
	// element.
	Element *ConstElementValue

	// Expression is the original FIDL expression given for the value,
	// included only when it meaningfully differs from the value.
	Expression string
}

func newConst(c fidlgen.Const, decls declMap, members memberMap) (*Const, error) {
	var kind TypeKind
	var typ string
	switch c.Type.Kind {
	case fidlgen.PrimitiveType:
		if c.Type.PrimitiveSubtype.IsFloat() {
			return nil, fmt.Errorf("floats are unsupported")
		}
		typ = string(c.Type.PrimitiveSubtype)
		switch typ {
		case string(fidlgen.Bool):
			kind = TypeKindBool
		case string(fidlgen.ZxExperimentalUsize64):
			kind = TypeKindSize
		default:
			kind = TypeKindInteger
		}
	case fidlgen.StringType:
		typ = string(fidlgen.StringType)
		kind = TypeKindString
	case fidlgen.IdentifierType:
		typ = string(c.Type.Identifier)
		switch decls[typ].(type) {
		case *Enum:
			kind = TypeKindEnum
		case *Bits:
			kind = TypeKindBits
		default:
			return nil, fmt.Errorf("%v has unsupported constant type: %s", c.Name, reflect.TypeOf(decls[typ]).Name())
		}
	default:
		return nil, fmt.Errorf("%v has unsupported constant type: %s", c.Name, c.Type.Kind)
	}

	val := getConstantValue(c.Value, kind)
	expr := c.Value.Expression
	var elVal *ConstElementValue
	switch c.Value.Kind {
	case fidlgen.LiteralConstant:
		// If the value matches the expression, there is no value in passing
		// the latter along.
		if expr == val || (kind == TypeKindString && expr == fmt.Sprintf("%q", val)) {
			expr = ""
		}
	case fidlgen.IdentifierConstant:
		valName, err := fidlgen.ReadName(string(c.Value.Identifier))
		if err != nil {
			return nil, err
		}
		declName, memberName := valName.SplitMember()
		elVal = &ConstElementValue{
			Decl: decls[declName.String()],
		}
		if memberName != "" {
			elVal.Member = members[valName.String()]
		}
		// The original expression doesn't matter if it gave another identifier.
		expr = ""
	case fidlgen.BinaryOperator:
		decl, ok := decls[typ]
		if !ok {
			break
		}
		elVal = &ConstElementValue{Decl: decl}
	}

	return &Const{
		decl:       newDecl(c),
		Kind:       kind,
		Type:       typ,
		Value:      val,
		Element:    elVal,
		Expression: expr,
	}, nil
}

// Gives the desired expression of a constant.
func getConstantValue(c fidlgen.Constant, kind TypeKind) string {
	// In the integer case, the original expression conveys more
	// information than the equivalent decimal value: if the author
	// intended for the number to be understood in binary or hex, then the
	// generated code should preserve that.
	if c.Kind == fidlgen.LiteralConstant && (kind == TypeKindInteger || kind == TypeKindSize) {
		return c.Expression
	}
	return c.Value
}

// ConstElementValue represents a constant value given by another FIDL element.
type ConstElementValue struct {
	// Decl either gives either another constant or the parent layout of an
	// enum or bits member.
	Decl

	// Member - if non-nil - gives the member of Decl() defining the associated
	// constant.
	Member
}

// Enum represents an FIDL enum declaration.
type Enum struct {
	decl

	// The primitive subtype underlying the Enum.
	Subtype fidlgen.PrimitiveSubtype

	// Members is the list of member values of the enum.
	Members []EnumMember
}

// EnumMember represents a FIDL enum value.
type EnumMember struct {
	member

	// Value is the member's value.
	Value string

	// Expression is the original FIDL expression given for the value,
	// included only when it meaningfully differs from the value.
	Expression string
}

func newEnum(enum fidlgen.Enum) (*Enum, error) {
	e := &Enum{
		decl:    newDecl(enum),
		Subtype: enum.Type,
	}
	for _, member := range enum.Members {
		val := getConstantValue(member.Value, TypeKindInteger)
		// If the value matches the expression, there is no value in passing
		// the latter along.
		expr := member.Value.Expression
		if expr == val {
			expr = ""
		}
		e.Members = append(e.Members, EnumMember{
			member:     newMember(member),
			Value:      val,
			Expression: expr,
		})
	}
	return e, nil
}

// Bits represents an FIDL bitset declaration.
type Bits struct {
	decl

	// The primitive subtype underlying the bitset.
	Subtype fidlgen.PrimitiveSubtype

	// Members is the list of member values of the bitset.
	Members []BitsMember
}

// BitsMember represents a FIDL enum value.
type BitsMember struct {
	member

	// Name is the name of the member.
	Name string

	// Index is the associated bit index.
	Index int
}

func newBits(bits fidlgen.Bits) (*Bits, error) {
	b := &Bits{
		decl:    newDecl(bits),
		Subtype: bits.Type.PrimitiveSubtype,
	}

	for _, member := range bits.Members {
		val, err := strconv.ParseUint(member.Value.Value, 10, 64)
		if err != nil {
			panic(fmt.Sprintf("%v member %s has bad value %q: %v", b.Name, member.Name, member.Value.Value, err))
		}

		b.Members = append(b.Members, BitsMember{
			member: newMember(member),
			Index:  log2(val),
		})
	}
	return b, nil
}

func log2(n uint64) int {
	if bits.OnesCount64(n) != 1 {
		panic(fmt.Sprintf("%d is not a power of two", n))
	}
	return bits.TrailingZeros64(n)
}

// TypeDescriptor gives a straightforward encoding of a type, accounting for
// any array nesting with a recursive pointer to a descriptor describing the
// element type.
type TypeDescriptor struct {
	// Type gives the full name of the type, except in the case of an array:
	// in that case, the array's element type is given by `.ElementType` and
	// its size is given by `.ElementCount`.
	Type string

	// Decl gives the associated declaration, if one exists.
	Decl Decl

	// Kind is the kind of the type.
	Kind TypeKind

	// ElementType gives the underlying element type in the case of an array.
	ElementType *TypeDescriptor

	// ElementCount gives the size of the associated array.
	ElementCount *int

	// The wire size of the type.
	Size int

	// Mutable gives whether this type is mutable.
	Mutable bool
}

// Represents an recursively-defined type, effectively abstracting
// fidlgen.Type and fidlgen.PartialTypeConstructor.
type recursiveType interface {
	GetKind() fidlgen.TypeKind
	// TODO(https://fxbug.dev/42057022): The presence of the declMap in the signature is
	// to account for yet another type alias IR deficiency: the IR loses
	// type information (e.g., size) about the value of the right-hand side, so
	// we use the map to look up previously processed declarations to make a
	// size determination.
	GetSize(decls declMap) int
	GetPrimitiveSubtype() fidlgen.PrimitiveSubtype
	GetIdentifierType() fidlgen.EncodedCompoundIdentifier
	GetElementType() recursiveType
	GetElementCount() *int
}

// Resolves a recursively-defined type into a type descriptor. This process
// requires a map of previously processed declarations for consulting, as well
// as map of type kinds to record those seen during the resolution.
func resolveType(typ recursiveType, attrs fidlgen.Attributes, decls declMap, typeKinds map[TypeKind]struct{}) (*TypeDescriptor, error) {
	desc := TypeDescriptor{Size: typ.GetSize(decls)}
	switch typ.GetKind() {
	case fidlgen.PrimitiveType:
		if typ.GetPrimitiveSubtype().IsFloat() {
			return nil, fmt.Errorf("floats are unsupported")
		}
		desc.Type = string(typ.GetPrimitiveSubtype())
		switch desc.Type {
		case string(fidlgen.Bool):
			desc.Kind = TypeKindBool
		case string(fidlgen.ZxExperimentalUsize64):
			desc.Kind = TypeKindSize
		default:
			desc.Kind = TypeKindInteger
		}
	case fidlgen.StringType:
		return nil, fmt.Errorf("strings are only supported as constants")
	case fidlgen.IdentifierType:
		desc.Type = string(typ.GetIdentifierType())
		desc.Decl = decls[desc.Type]
		switch desc.Decl.(type) {
		case *Enum:
			desc.Kind = TypeKindEnum
		case *Bits:
			desc.Kind = TypeKindBits
		case *Struct:
			desc.Kind = TypeKindStruct
		case *Overlay:
			desc.Kind = TypeKindOverlay
		case *Alias:
			desc.Kind = TypeKindAlias
		default: // TODO(https://fxbug.dev/42057859): Skip if unknown.
			return nil, nil
		}

	case fidlgen.ArrayType:
		desc.Kind = TypeKindArray
		desc.ElementCount = typ.GetElementCount()
		nested, err := resolveType(typ.GetElementType(), fidlgen.Attributes{}, decls, typeKinds)
		if err != nil {
			return nil, err
		}
		if nested == nil { // TODO(https://fxbug.dev/42057859): Skip if unknown.
			return nil, nil
		}
		desc.ElementType = nested
	case fidlgen.StringArray:
		desc.Kind = TypeKindStringArray
		desc.ElementCount = typ.GetElementCount()
	case fidlgen.ZxExperimentalPointerType, fidlgen.VectorType:
		if typ.GetKind() == fidlgen.ZxExperimentalPointerType {
			desc.Kind = TypeKindPointer
		} else {
			desc.Kind = TypeKindVector
		}

		// TODO(https://fxbug.dev/42057022): Temporary contriving of alias name currently
		// lost in the IR.
		//
		// See the definition of @embedded_alias in //zircon/vdso/README.md for
		// more detail.
		elementType := typ.GetElementType()
		if attr, ok := attrs.LookupAttribute("embedded_alias"); ok {
			elementType = fidlgenType(fidlgen.Type{
				Kind:       fidlgen.IdentifierType,
				Identifier: fidlgen.EncodedCompoundIdentifier(attr.Args[0].ValueString()),
			})
		}
		nested, err := resolveType(elementType, fidlgen.Attributes{}, decls, typeKinds)
		if err != nil {
			return nil, err
		}
		if nested == nil { // TODO(https://fxbug.dev/42057859): Skip if unknown.
			return nil, nil
		}
		desc.ElementType = nested

		if attrs.HasAttribute("voidptr") {
			if nested.Type != string(fidlgen.Uint8) {
				return nil, fmt.Errorf("@voidptr may only annotate `experimental_pointer` for a pointee type of `uint8`")
			}
			if desc.Kind == TypeKindPointer {
				desc.Kind = TypeKindVoidPointer
			} else {
				desc.Kind = TypeKindVoidVector
			}
		}
		if attrs.HasAttribute("size32") {
			if desc.Kind == TypeKindVector {
				desc.Kind = TypeKindVector32
			} else if desc.Kind == TypeKindVoidVector {
				desc.Kind = TypeKindVoidVector32
			} else {
				return nil, fmt.Errorf("@size32 may only annotate `vector`s")
			}
		}
	case fidlgen.HandleType:
		desc.Kind = TypeKindHandle
		desc.Type = string(typ.GetIdentifierType())
		desc.Decl = decls[desc.Type]
	default: // TODO(https://fxbug.dev/42057859): Skip if unknown.
		return nil, nil
	}

	typeKinds[desc.Kind] = struct{}{}
	return &desc, nil
}

// A thin wrapper implementing `recursiveType`.
type fidlgenType fidlgen.Type

func (typ fidlgenType) GetKind() fidlgen.TypeKind { return typ.Kind }

func (typ fidlgenType) GetSize(_ declMap) int { return typ.TypeShapeV2.InlineSize }

func (typ fidlgenType) GetPrimitiveSubtype() fidlgen.PrimitiveSubtype {
	return typ.PrimitiveSubtype
}

func (typ fidlgenType) GetIdentifierType() fidlgen.EncodedCompoundIdentifier {
	if typ.ResourceIdentifier != "" {
		return fidlgen.EncodedCompoundIdentifier(typ.ResourceIdentifier)
	}
	return typ.Identifier
}

func (typ fidlgenType) GetElementType() recursiveType {
	if typ.PointeeType != nil {
		return fidlgenType(*typ.PointeeType)
	}
	return fidlgenType(*typ.ElementType)
}

func (typ fidlgenType) GetElementCount() *int { return typ.ElementCount }

// Struct represents a FIDL struct declaration.
type Struct struct {
	decl

	// Size is the size of the struct, including padding.
	Size int

	// Members is the list of the members of the layout.
	Members []StructMember

	// HasPadding gives whether the struct has any implicit padding on the wire.
	HasPadding bool

	// Whether the struct was formally declared; if false, it was synthesized
	// or implicitly declared (as is in the case of a protocol method
	// request/response payload).
	synthesized bool

	// TODO(https://fxbug.dev/42061412): wrappedReturn indicates that this struct defines
	// the singleton, response body of a protocol method used to define a
	// syscall, and further that the return type of that syscall is actually
	// the type of the wrapped parameter. The reason for this workaround is
	// that protocol methods must return a struct.
	//
	// See the definition of @wrapped_return in //zircon/vdso/README.md
	wrappedReturn bool
}

// StructMember represents a FIDL struct member.
type StructMember struct {
	member

	// Type describes the type of the member.
	Type TypeDescriptor

	// Offset is the offset of the field.
	Offset int

	// TODO(https://fxbug.dev/42061412): The following attributes may annotate "syscall"
	// request and response structs. While we have to synthesize syscall
	// information manually, we record these values here during member
	// processing for later syscall processing.
	//
	// See the definitions of @inout, @out, and @release in
	// //zircon/vdso/README.md.
	inout   bool
	out     bool
	release bool
}

func newStruct(strct fidlgen.Struct, decls declMap, typeKinds map[TypeKind]struct{}) (*Struct, error) {
	s := &Struct{
		decl:          newDecl(strct),
		Size:          strct.TypeShapeV2.InlineSize,
		HasPadding:    strct.TypeShapeV2.HasPadding,
		synthesized:   strct.IsAnonymous(),
		wrappedReturn: strct.GetAttributes().HasAttribute("wrapped_return"),
	}
	for _, member := range strct.Members {
		attrs := member.GetAttributes()

		// TODO(https://fxbug.dev/42057022): For struct members, we have the `MaybeFromAlias`
		// field as a workaround for recovering any alias name.
		memberType := recursiveType(fidlgenType(member.Type))
		if member.MaybeFromAlias != nil {
			memberType = recursiveType(fidlgenTypeCtor(*member.MaybeFromAlias))
		}
		typ, err := resolveType(memberType, attrs, decls, typeKinds)
		if err != nil {
			return nil, fmt.Errorf("%s.%s: failed to derive type: %w", s.Name, member.Name, err)
		}
		if typ == nil { // TODO(https://fxbug.dev/42057859): Skip if unknown.
			continue
		}
		s.Members = append(s.Members, StructMember{
			member: newMember(member),
			Type:   *typ,
			Offset: member.FieldShapeV2.Offset,

			// TODO(https://fxbug.dev/42061412): Eventually the IR will effectively
			// surface this information directly in the syscall-related
			// elements: at that point this can be removed.
			inout:   attrs.HasAttribute("inout"),
			out:     attrs.HasAttribute("out"),
			release: attrs.HasAttribute("release"),
		})
	}

	// Syscall parameters are encoded in synthesized/anonymous structs. While
	// we do not want conventional bindings for these structs, we do want to
	// make them available to later syscall processing: record them here but
	// return nothing.
	if s.synthesized {
		decls[s.Name.String()] = s
		return nil, nil
	}
	return s, nil
}

// Overlay represents a FIDL overlay declaration.
type Overlay struct {
	decl

	// MaxVariantSize is the maximum wire size of the overlay's variant types,
	// including padding.
	MaxVariantSize int

	Variants []OverlayVariant
}

// Size gives the wire size of the overlay.
func (o Overlay) Size() int {
	return 8 + o.MaxVariantSize // sizeof(discriminant) + max. variant size.
}

// OverlayVariant represents a FIDL overlay variant (i.e., member).
type OverlayVariant struct {
	member

	Type TypeDescriptor

	// Discriminant is the ordinal uniquely identifying the variant.
	Discriminant int
}

func newOverlay(overlay fidlgen.Overlay, decls declMap, typeKinds map[TypeKind]struct{}) (*Overlay, error) {
	o := &Overlay{
		decl:           newDecl(overlay),
		MaxVariantSize: overlay.TypeShapeV2.InlineSize - 8,
		Variants:       make([]OverlayVariant, len(overlay.Members)),
	}
	for i, member := range overlay.Members {
		attrs := member.GetAttributes()

		// TODO(https://fxbug.dev/42057022): For overlay members, we have the
		// `MaybeFromAlias` field as a workaround for recovering any alias name.
		memberType := recursiveType(fidlgenType(member.Type))
		if member.MaybeFromAlias != nil {
			memberType = recursiveType(fidlgenTypeCtor(*member.MaybeFromAlias))
		}
		typ, err := resolveType(memberType, attrs, decls, typeKinds)
		if err != nil {
			return nil, fmt.Errorf("%s.%s: failed to derive type: %w", o.Name, member.Name, err)
		}
		if err != nil || typ == nil {
			return nil, fmt.Errorf("%s.%s: failed to derive type: %w", o.Name, member.Name, err)
		}
		o.Variants[i] = OverlayVariant{
			member:       newMember(member),
			Type:         *typ,
			Discriminant: member.Ordinal,
		}
	}
	return o, nil
}

// Alias represents a FIDL type alias declaration.
type Alias struct {
	decl

	// Value is the type under alias (i.e., the right-hand side of the
	// declaration).
	Value TypeDescriptor
}

func newAlias(alias fidlgen.Alias, decls declMap, typeKinds map[TypeKind]struct{}) (*Alias, error) {
	unresolved := fidlgenTypeCtor(alias.PartialTypeConstructor)
	typ, err := resolveType(unresolved, alias.GetAttributes(), decls, typeKinds)
	if err != nil {
		return nil, err
	}
	if typ == nil { // TODO(https://fxbug.dev/42057859): Skip if unknown.
		return nil, nil
	}
	return &Alias{
		decl:  newDecl(alias),
		Value: *typ,
	}, nil
}

// An implementation of `recursiveType`.
type fidlgenTypeCtor fidlgen.PartialTypeConstructor

func (ctor fidlgenTypeCtor) GetKind() fidlgen.TypeKind {
	if _, err := fidlgen.ReadName(string(ctor.Name)); err == nil {
		return fidlgen.IdentifierType
	}

	switch string(ctor.Name) {
	case string(fidlgen.Bool),
		string(fidlgen.Int8),
		string(fidlgen.Int16),
		string(fidlgen.Int32),
		string(fidlgen.Int64),
		string(fidlgen.Uint8),
		string(fidlgen.Uint16),
		string(fidlgen.Uint32),
		string(fidlgen.Uint64),
		string(fidlgen.Float32),
		string(fidlgen.Float64),
		string(fidlgen.ZxExperimentalUchar),
		string(fidlgen.ZxExperimentalUsize64),
		string(fidlgen.ZxExperimentalUintptr64):
		return fidlgen.PrimitiveType
	case "array":
		return fidlgen.ArrayType
	default:
		panic(fmt.Sprintf("unsupported type: %s", ctor.Name))
	}
}

func (ctor fidlgenTypeCtor) GetSize(decls declMap) int {
	switch ctor.GetKind() {
	case fidlgen.IdentifierType:
		d := decls[string(ctor.Name)]
		switch d.(type) {
		case *Enum:
			return primitiveWireSize(d.(*Enum).Subtype)
		case *Bits:
			return primitiveWireSize(d.(*Bits).Subtype)
		case *Struct:
			return d.(*Struct).Size
		case *Alias:
			return d.(*Alias).Value.Size
		case *Handle:
			return 4
		default:
			// The above declarations are the only ones that should appear as
			// an alias value.
			panic(fmt.Sprintf("unknown alias value kind %s: %#v", ctor.GetKind(), ctor))
		}
	case fidlgen.PrimitiveType:
		return primitiveWireSize(ctor.GetPrimitiveSubtype())
	case fidlgen.ArrayType:
		return *ctor.GetElementCount() * ctor.GetElementType().GetSize(decls)
	default:
		panic(fmt.Sprintf("unsupported type: %s", ctor.Name))
	}
}

func (ctor fidlgenTypeCtor) GetPrimitiveSubtype() fidlgen.PrimitiveSubtype {
	return fidlgen.PrimitiveSubtype(ctor.Name)
}

func (ctor fidlgenTypeCtor) GetIdentifierType() fidlgen.EncodedCompoundIdentifier {
	return ctor.Name
}

func (ctor fidlgenTypeCtor) GetElementType() recursiveType {
	if len(ctor.Args) == 0 {
		return nil
	}
	// TODO(https://fxbug.dev/42156522): This list appears to always be empty or a
	// singleton (and its unclear what further arguments would mean).
	return fidlgenTypeCtor(ctor.Args[0])
}

func (ctor fidlgenTypeCtor) GetElementCount() *int {
	if ctor.MaybeSize == nil {
		return nil
	}
	count, err := strconv.Atoi(ctor.MaybeSize.Value)
	if err != nil {
		panic(fmt.Sprintf("could not interpret %s as an int", ctor.MaybeSize.Value))
	}
	return &count
}

func primitiveWireSize(typ fidlgen.PrimitiveSubtype) int {
	switch typ {
	case fidlgen.Bool, fidlgen.Int8, fidlgen.Uint8,
		fidlgen.ZxExperimentalUchar:
		return 1
	case fidlgen.Int16, fidlgen.Uint16:
		return 2
	case fidlgen.Int32, fidlgen.Uint32:
		return 4
	case fidlgen.Int64, fidlgen.Uint64,
		fidlgen.ZxExperimentalUintptr64, fidlgen.ZxExperimentalUsize64:
		return 8
	default:
		panic(fmt.Sprintf("unknown primitive type: %s", typ))
	}
}

// Handle represents a Zircon handle, as represented by a FIDL resource declaration.
//
// There is little value in modeling things in terms of generic resources, as
// the construction is already arbitrarily constrained to handle-like things
// and handles are currently the only use-case.
type Handle struct {
	decl
}

func newHandle(resource fidlgen.Resource, typeKinds map[TypeKind]struct{}) (*Handle, error) {
	name := fidlgen.MustReadName(string(resource.Name)).DeclarationName()
	if name != "Handle" {
		return nil, fmt.Errorf("resource declarations must be named \"Handle\", not %s", name)
	}
	typeKinds[TypeKindInteger] = struct{}{}
	return &Handle{decl: newDecl(resource)}, nil
}

// SyscallFamily represents a logical family of syscalls, usually corresponding
// to a kernel object and the noun phrase namespacing in
// `zx_${noun_phrase}_${verb_phrase}`.
//
// The interpretation here relies upon the conventions detailed in
// //zircon/vdso/README.md.
type SyscallFamily struct {
	decl

	// Syscalls gives the syscalls that comprise the family.
	Syscalls []Syscall
}

// SyscallCategory gives a categorization of syscalls.
type SyscallCategory string

const (
	// SyscallCategoryBasic indicates the basic, unqualified category of
	// syscalls (which is the majority).
	SyscallCategoryBasic SyscallCategory = ""

	// SyscallCategoryInternal indicates that the syscall is not part of public
	// ABI.
	//
	// See the definition of @internal in //zircon/vdso/README.md.
	SyscallCategoryInternal SyscallCategory = "internal"

	// SyscallCategoryNext indicates that the syscall is in the process of
	// being stabilized and is not yet a part of public ABI.
	//
	// See the definition of @next in //zircon/vdso/README.md.
	SyscallCategoryNext SyscallCategory = "next"

	// SyscallCategoryTest1 is a category relevant only to testing.
	//
	// See the definition of @test_category1 in //zircon/vdso/README.md.
	SyscallCategoryTest1 SyscallCategory = "test_category1"

	// SyscallCategoryTest2 is a category relevant only to testing.
	//
	// See the definition of @test_category2 in //zircon/vdso/README.md.
	SyscallCategoryTest2 SyscallCategory = "test_category2"
)

// Syscall corresponds to an individual syscall.
//
// `Name` includes the family namespacing of the syscall (i.e., "PortWait"
// instead of just "Wait").
type Syscall struct {
	member

	// Category gives the syscall's category.
	Category SyscallCategory

	// Blocking indicates a blocking call.
	//
	// See the definition of @blocking in //zircon/vdso/README.md.
	Blocking bool

	// NoReturn indicates that the sycall does not return.
	//
	// See the definition of @noreturn in //zircon/vdso/README.md.
	NoReturn bool

	// VdsoCall indicates that the syscall is not a true syscall and is actually
	// implemented within the vDSO.
	//
	// See the definition of @vdsocall in //zircon/vdso/README.md.
	VdsoCall bool

	// Const, which only applies to VdsoCall syscalls, indicates that the
	// function is "const" in the sense of `__attribute__((__const__))`.
	//
	// See the definition of @const in //zircon/vdso/README.md.
	Const bool

	// Whether the syscall is defined solely for tests.
	//
	// See the definition of @testonly in //zircon/vdso/README.md.
	Testonly bool

	// Parameters gives the list of parameters in the C vDSO signature of the
	// syscall in order.
	Parameters []SyscallParameter

	// ReturnType gives the return type of the C vDSO signature.
	ReturnType *TypeDescriptor
}

func (syscall Syscall) IsInternal() bool      { return syscall.Category == SyscallCategoryInternal }
func (syscall Syscall) IsNext() bool          { return syscall.Category == SyscallCategoryNext }
func (syscall Syscall) IsTestCategory1() bool { return syscall.Category == SyscallCategoryTest1 }
func (syscall Syscall) IsTestCategory2() bool { return syscall.Category == SyscallCategoryTest2 }

// ParameterOrientation describes whether the parameter serves as input,
// output, or both.
type ParameterOrientation int

const (
	ParameterOrientationIn ParameterOrientation = iota
	ParameterOrientationOut
	ParameterOrientationInOut
)

// ParameterTag gives additional, miscellaneous information about a syscall
// parameter, informing how it should be processed.
type ParameterTag int

const (
	// ParameterTagDecayedFromVector gives whether this parameter derives
	// from one given originally as a `vector`. This should only be the case
	// when the associated parameter gives a 'pointer' or a 'length'.
	ParameterTagDecayedFromVector ParameterTag = iota

	// ParameterTagUncheckedHandle indicates that associated handle parameter
	// may be left in a state (e.g., consumed) dependent on the success of the
	// syscall.
	ParameterTagUncheckedHandle

	// ParameterTagReleasedHandle indicates that the associated handle
	// parameter is consumed by its syscall.
	ParameterTagReleasedHandle
)

// SyscallParameter represents a C syscall parameter.
type SyscallParameter struct {
	member

	// Type gives the type of the parameter.
	Type TypeDescriptor

	// Orientation gives whether this is an in, out, or in-out parameter.
	Orientation ParameterOrientation

	// Size is the size of the parameter.
	Size int

	// Tags gives additional metadata about the parameter.
	Tags map[ParameterTag]struct{}
}

// HasTag checks whether the parameter has a given tag. This method is a safety
// and convenience accessor around SyscallParameter.Tags that handles the case
// of an uninitialized nil value.
func (param SyscallParameter) HasTag(tag ParameterTag) bool {
	if param.Tags == nil {
		return false
	}
	_, ok := param.Tags[tag]
	return ok
}

// SetTag sets a given tag on the parameter. This method is a safety and
// convenience accessor around SyscallParameter.Tags that handles the case of
// an uninitialized nil value.
func (param *SyscallParameter) SetTag(tag ParameterTag) {
	if param.Tags == nil {
		param.Tags = make(map[ParameterTag]struct{})
	}
	param.Tags[tag] = struct{}{}
}

func (param SyscallParameter) IsStrictInput() bool {
	return param.Orientation == ParameterOrientationIn
}

func (param SyscallParameter) IsStrictOutput() bool {
	return param.Orientation == ParameterOrientationOut
}

func newSyscallFamily(protocol fidlgen.Protocol, decls declMap) (*SyscallFamily, error) {
	if protocol.OverTransport() != "Syscall" {
		return nil, nil
	}
	family := &SyscallFamily{decl: newDecl(protocol)}
	// This gives whether the family protocol name actually meaningfully
	// signals the official namespacing of the syscalls. If the attribute is
	// present, the name is arbitrary and the syscall member names already
	// include the proper namespacing.
	//
	// See @no_protocol_prefix in //zircon/vdso/README.md
	_, noProtocolPrefix := protocol.LookupAttribute("no_protocol_prefix")
	for _, method := range protocol.Methods {

		syscall := Syscall{
			member:   newMember(method),
			Category: SyscallCategoryBasic,
		}
		if !noProtocolPrefix {
			syscall.member.Name = family.Name.DeclarationName() + syscall.member.Name
		}
		for _, cat := range []SyscallCategory{
			SyscallCategoryInternal,
			SyscallCategoryNext,
			SyscallCategoryTest1,
			SyscallCategoryTest2,
		} {
			if _, ok := method.LookupAttribute(fidlgen.Identifier(cat)); !ok {
				continue
			}
			if syscall.Category != SyscallCategoryBasic {
				return nil, fmt.Errorf("syscall %s cannot be annotated with both @%s and @%s", syscall.Name, syscall.Category, cat)
			}
			syscall.Category = cat
		}

		_, syscall.Blocking = method.LookupAttribute("blocking")
		_, syscall.NoReturn = method.LookupAttribute("noreturn")
		_, syscall.Const = method.LookupAttribute("const")
		_, syscall.Testonly = method.LookupAttribute("testonly")
		_, syscall.VdsoCall = method.LookupAttribute("vdsocall")

		// @const must be paired with @vdsocall.
		if syscall.Const && !syscall.VdsoCall {
			return nil, fmt.Errorf("annotation @const on syscall %s must be paired with @vdsocall", syscall.Name)
		}

		// @test_category{1,2} must be paired with @testonly
		if (syscall.IsTestCategory1() || syscall.IsTestCategory2()) && !syscall.Testonly {
			return nil, fmt.Errorf("annotation @%s on syscall %s must be paired with @testonly", syscall.Category, syscall.Name)
		}

		// If the method declares an error type, then that must be our C return
		// type.
		if method.ErrorType != nil {
			var err error
			syscall.ReturnType, err = resolveType(fidlgenType(*method.ErrorType), fidlgen.Attributes{}, decls, make(map[TypeKind]struct{}))
			if err != nil {
				return nil, err
			}

			// TODO(https://fxbug.dev/42057022, https://fxbug.dev/42065140): The name of an aliased
			// error type does not yet survive into the IR (just the full
			// resolution). So, to account for the major case of wanting to use
			// `zx/Status` as an error type - while in its alias form - we
			// hackily replace any int32 error specifications with a `zx/Status`
			// int32 alias if present in the library.
			//
			// Once one of these bugs is fixed, this workaround can be removed.
			if syscall.ReturnType.Kind == TypeKindInteger && syscall.ReturnType.Type == string(fidlgen.Int32) {
				if status, ok := decls["zx/Status"]; ok {
					int32Type := TypeDescriptor{
						Type: string(fidlgen.Int32),
						Kind: TypeKindInteger,
						Size: 4,
					}
					if alias, ok := status.(*Alias); ok && alias.Value == int32Type {
						syscall.ReturnType = &TypeDescriptor{
							Type: "zx/Status",
							Kind: TypeKindAlias,
							Decl: status,
							Size: 4,
						}
					}
				}
			}
		}

		// Aggregate parameters from both the request and response structs, as
		// 'out' parameters may appear in the request struct. With request
		// struct members first, the default ordering results in the desired
		// order of parameters in the vDSO intertace.
		processParams := func(typ fidlgen.Type, request bool) {
			what := "request"
			if !request {
				what = "response"
			}
			if typ.Kind != fidlgen.IdentifierType {
				panic(fmt.Sprintf("method %s type is not given by an indentifier", what))
			}
			ident, ok := decls[string(typ.Identifier)]
			if !ok {
				panic(fmt.Sprintf("unknown method %s type: %s", what, typ.Identifier))
			}
			s, ok := ident.(*Struct)
			if !ok {
				panic(fmt.Sprintf("method %s type %s is not a struct", what, typ.Identifier))
			}

			if s.wrappedReturn {
				if request {
					panic(fmt.Sprintf("@wrapped_return cannot annotate a request payload: %s", syscall.Name))
				}
				if syscall.ReturnType != nil {
					panic(fmt.Sprintf("@wrapped_return cannot be paired with `error`: %s", syscall.Name))
				}
				if len(s.Members) != 1 {
					panic(fmt.Sprintf("@wrapped_return must annotate a singleton struct: %s", syscall.Name))
				}
				syscall.ReturnType = &s.Members[0].Type
				return
			}

			// If we are (a) processing the outputs, (b) the syscall has no
			// return error type, and (c) we have formally declared the struct
			// of outputs as a proper declaration, then that declared output
			// struct should be regarded as the syscall's return type.
			if !request && syscall.ReturnType == nil && !s.synthesized {
				syscall.ReturnType = &TypeDescriptor{
					Type: string(typ.Identifier),
					Kind: TypeKindStruct,
					Decl: ident,
					Size: s.Size,
				}
				return
			}

			for _, m := range s.Members {
				param := SyscallParameter{
					member:      m.member,
					Type:        m.Type,
					Orientation: ParameterOrientationIn,
				}
				if !request || m.out {
					param.Orientation = ParameterOrientationOut
				} else if m.inout {
					param.Orientation = ParameterOrientationInOut
				}
				kind := param.Type.Kind
				if !param.IsStrictInput() && (kind.IsVectorLike() || kind.IsPointerLike()) {
					param.Type.ElementType.Mutable = true
				}

				switch kind {
				case TypeKindHandle, TypeKindPointer, TypeKindVector, TypeKindVector32:
					if kind != TypeKindHandle && param.Type.ElementType.Kind != TypeKindHandle {
						break
					}
					if method.GetAttributes().HasAttribute("handle_unchecked") {
						param.SetTag(ParameterTagUncheckedHandle)
					} else if m.release {
						param.SetTag(ParameterTagReleasedHandle)
					}
				}

				// Vector parameters decay into separate pointer and length
				// parameters.
				if kind.IsVectorLike() {
					pointer := param
					pointer.Type.Size = 8
					pointer.SetTag(ParameterTagDecayedFromVector)
					if kind.IsVoidVectorLike() {
						pointer.Type.Kind = TypeKindVoidPointer
					} else {
						pointer.Type.Kind = TypeKindPointer
					}

					length := SyscallParameter{
						member: m.member,
						// We always regard lengths as inputs.
						Orientation: ParameterOrientationIn,
					}
					length.SetTag(ParameterTagDecayedFromVector)

					// Usually arrays with names that end in "s" are plurals,
					// and usually arrays that refer to a plurality of objects
					// deal in counts and not sizes. This is a cheesy
					// heuristic, but it matches our reality close enough.
					if strings.HasSuffix(param.Name, "s") {
						length.Name = "num_" + length.Name
					} else {
						length.Name += "_size"
					}

					if kind.IsVector32Like() {
						length.Type = TypeDescriptor{
							Kind: TypeKindInteger,
							Type: string(fidlgen.Uint32),
							Size: 4,
						}
					} else {
						length.Type = TypeDescriptor{
							Kind: TypeKindSize,
							Type: string(fidlgen.ZxExperimentalUsize64),
							Size: 8,
						}
					}
					syscall.Parameters = append(syscall.Parameters, pointer, length)
				} else {
					syscall.Parameters = append(syscall.Parameters, param)
				}
			}

		}

		if method.RequestPayload != nil {
			processParams(*method.RequestPayload, true)
		}
		// `ValueType` gives the desired output struct when `error` is
		// specified (with `ResponsePayload` giving a union of both that and
		// the error type).
		if method.ValueType != nil {
			processParams(*method.ValueType, false)
		} else if method.ResponsePayload != nil {
			processParams(*method.ResponsePayload, false)
		}

		family.Syscalls = append(family.Syscalls, syscall)
	}
	return family, nil
}
