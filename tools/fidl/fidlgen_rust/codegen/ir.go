// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package codegen

import (
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"
	"unicode"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type EncodedCompoundIdentifier = fidlgen.EncodedCompoundIdentifier

type Type struct {
	// TODO(https://fxbug.dev/42156522): Remove Resourceness once stored on fidlgen.Type.
	fidlgen.Resourceness

	// Information extracted from fidlgen.Type.
	Kind             fidlgen.TypeKind
	Nullable         bool
	PrimitiveSubtype fidlgen.PrimitiveSubtype
	ElementType      *Type
	Identifier       EncodedCompoundIdentifier
	DeclType         fidlgen.DeclType

	// The marker type that implements fidl::encoding::Type.
	FidlTemplate string
	// The associated type fidl::encoding::Type::Owned.
	Owned string
	// The type to use when this occurs as a method parameter.
	// TODO(https://fxbug.dev/42073194): Once the transition to the new types if complete,
	// document this as being {Value,Resource}Type::Borrowed.
	Param string
}

func (t *Type) Fidl(dialect string) string {
	return strings.Replace(t.FidlTemplate, "#DIALECT", dialect, -1)
}

type Alias struct {
	fidlgen.Alias
	Name string
	Type Type
}

type Bits struct {
	fidlgen.Bits
	Name           string
	UnderlyingType string
	Members        []BitsMember
}

type BitsMember struct {
	fidlgen.BitsMember
	Name  string
	Value string
}

type Const struct {
	fidlgen.Const
	Name  string
	Type  string
	Value string
}

type Enum struct {
	fidlgen.Enum
	Name           string
	UnderlyingType string
	Members        []EnumMember
	// Member name with the minimum value, used as an arbitrary default value
	// in Decode::new_empty for strict enums.
	MinMember string
}

type EnumMember struct {
	fidlgen.EnumMember
	Name  string
	Value string
}

type Union struct {
	fidlgen.Union
	Derives      derives
	ECI          EncodedCompoundIdentifier
	Name         string
	Members      []UnionMember
	Serializable fidlgen.Serializable
	Sensitive    bool
}

type UnionMember struct {
	fidlgen.UnionMember
	Type      Type
	Name      string
	Ordinal   int
	Sensitive bool
}

type Struct struct {
	fidlgen.Struct
	ECI                       EncodedCompoundIdentifier
	Derives                   derives
	Name                      string
	Members                   []StructMember
	PaddingMarkersV2          []fidlgen.PaddingMarker
	FlattenedPaddingMarkersV2 []fidlgen.PaddingMarker
	SizeV2                    int
	AlignmentV2               int
	HasPadding                bool
	// True if the struct should be encoded and decoded by memcpy.
	UseFidlStructCopy bool
	Serializable      fidlgen.Serializable
	Sensitive         bool
}

type StructMember struct {
	fidlgen.StructMember
	Type      Type
	Name      string
	OffsetV2  int
	Sensitive bool
}

type Table struct {
	fidlgen.Table
	Derives      derives
	ECI          EncodedCompoundIdentifier
	Name         string
	Members      []TableMember
	Serializable fidlgen.Serializable
	Sensitive    bool
}

func (t *Table) ReversedMembers() []TableMember {
	var r []TableMember
	for i := len(t.Members) - 1; i >= 0; i-- {
		r = append(r, t.Members[i])
	}
	return r
}

type TableMember struct {
	fidlgen.TableMember
	Type      Type
	Name      string
	Ordinal   int
	Sensitive bool
}

// Protocol is the definition of a protocol in the library being compiled.
type Protocol struct {
	// Raw JSON IR data about this protocol. Embedded to provide access to
	// fields common to all bindings.
	fidlgen.Protocol
	// Compound identifier referring to this protocol.
	ECI EncodedCompoundIdentifier
	// String to use for ProtocolMarker::DEBUG_NAME.
	DebugName string
	// True if the protocol is marked @discoverable.
	Discoverable bool
	// Name of the protocol's Marker struct.
	Marker string
	// Name of the protocol's Proxy struct.
	Proxy string
	// Name of the protocol's ProxyInterface trait.
	ProxyInterface string
	// Name of the protocol's SynchronousProxy struct.
	SynchronousProxy string
	// Name of the protocol's Request enum.
	Request string
	// Name of the protocol's RequestStream struct.
	RequestStream string
	// Name of the protocol's Event enum.
	Event string
	// Name of the protocol's EventStream struct.
	EventStream string
	// Name of the protocol's ControlHandle struct.
	ControlHandle string
	// Name of the namespace of protocol ordinal constants.
	Ordinals string
	// List of methods that are part of this protocol.
	Methods []Method
}

// Method is a method defined in a protocol.
type Method struct {
	// Raw JSON IR data about this method. Embedded to provide access to fields
	// common to all bindings.
	fidlgen.Method
	// Name of the method converted to snake_case. Used when generating
	// rust-methods associated with this method, such as proxy methods and
	// encoder methods.
	Name string
	// Name of the method converted to UPPER_SNAKE_CASE. Used to generate the
	// ordinal constants.
	UpperSnakeName string
	// Name of the method converted to CamelCase. Used when generating
	// rust-types associated with this method, such as responders.
	CamelName string
	// Name of the method's Responder struct.
	Responder string
	// Name of the method's ResponseFut type in the protocol's ProxyInterface trait.
	ResponseFut string
	// FullyQualifiedName is the fully qualified name of the FIDL method.
	FullyQualifiedName string

	Request  Payload
	Response Payload
}

// DynamicFlags gets rust code for the DynamicFlags value that should be set for
// a call to this method.
func (m *Method) DynamicFlags() string {
	if m.IsStrict() {
		return "fidl::encoding::DynamicFlags::empty()"
	}
	return "fidl::encoding::DynamicFlags::FLEXIBLE"
}

func (m *Method) ApiCoverageMethodAnnotation() string {
	return "// api-coverage-fidl-name:" + m.FullyQualifiedName
}

func (m *Method) ApiCoverageEndMethodAnnotation() string {
	return "// api-coverage-fidl-name:" + m.FullyQualifiedName + ":end"
}

// A method request or response.
type Payload struct {
	// The fidl::encoding::Type type. It can be fidl::encoding::EmptyPayload,
	// a struct, table, or union, or (only for two-way responses) one of
	// fidl::encoding::{Result,Flexible,FlexibleResult}Type.
	FidlTypeTemplate string
	// Equivalent to <FidlType as fidl::encoding::Type>::Owned.
	OwnedType string
	// The user-facing owned type. This is derived from OwnedType by removing
	// flexible wrappers (if any) and flattening structs to tuples.
	TupleType string
	// For methods that use error syntax, TupleType is an alias that refers
	// to TupleTypeAliasRhs. Otherwise, TupleTypeAliasRhs is empty.
	// TODO(https://fxbug.dev/42073194): Remove.
	TupleTypeAliasRhs string
	// Parameters for sending this payload. For an empty payload, this is nil.
	// For a struct without error syntax, it contains one element per struct
	// field. Otherwise, it contains a single element (table, union, or Result).
	Parameters []Parameter
	// Assuming that names from Parameters are in scope, EncodeExpr is an
	// expression of a type implementing fidl::encoding::Encode<FidlType>.
	EncodeExpr string
	// ConvertToTuple converts an expression to TupleType. If OwnedType is
	// fidl::encoding::Flexible or fidl::encoding::FlexibleResult, expects the
	// input variable to be the Ok return value of .into_result(). Otherwise,
	// expects the input variable to be of type OwnedType.
	// TODO(https://fxbug.dev/42073194): Remove.
	ConvertToTuple func(string) string
	// Like ConvertToTuple, but uses names from Parameters for inclusion in a
	// struct. Examples for ConvertToFields("v"):
	//     ""
	//     "param1: v.param1, param2: v.param2,"
	//     "result: v.map(|x| x.param1),"
	// TODO(https://fxbug.dev/42073194): Remove.
	ConvertToFields func(string) string
}

func (p *Payload) FidlType(dialect string) string {
	return strings.Replace(p.FidlTypeTemplate, "#DIALECT", dialect, -1)
}

// A parameter in a method request or response.
type Parameter struct {
	// Snake-case parameter name.
	Name string
	// Parameter type.
	Type string
	// Type to use when storing the parameter in an owning data structure.
	OwnedType string
	// Set only if this parameter corresponds to a struct member.
	StructMemberType *Type
}

type Service struct {
	fidlgen.Service
	Name        string
	Members     []ServiceMember
	ServiceName string
}

type ServiceMember struct {
	fidlgen.ServiceMember
	ProtocolType string
	Name         string
	CamelName    string
	SnakeName    string
}

type Root struct {
	Experiments  fidlgen.Experiments
	ExternCrates []string
	Aliases      []Alias
	Bits         []Bits
	Consts       []Const
	Enums        []Enum
	Structs      []Struct
	Unions       []Union
	Tables       []Table
	Protocols    []Protocol
	Services     []Service
}

func (r *Root) findProtocol(eci EncodedCompoundIdentifier) *Protocol {
	for i := range r.Protocols {
		if r.Protocols[i].ECI == eci {
			return &r.Protocols[i]
		}
	}
	return nil
}

func (r *Root) findStruct(eci EncodedCompoundIdentifier) *Struct {
	for i := range r.Structs {
		if r.Structs[i].ECI == eci {
			return &r.Structs[i]
		}
	}
	return nil
}

func (r *Root) findTable(eci EncodedCompoundIdentifier) *Table {
	for i := range r.Tables {
		if r.Tables[i].ECI == eci {
			return &r.Tables[i]
		}
	}
	return nil
}

func (r *Root) findUnion(eci EncodedCompoundIdentifier) *Union {
	for i := range r.Unions {
		if r.Unions[i].ECI == eci {
			return &r.Unions[i]
		}
	}
	return nil
}

var reservedWords = map[string]struct{}{
	"as":       {},
	"box":      {},
	"break":    {},
	"const":    {},
	"continue": {},
	"crate":    {},
	"else":     {},
	"enum":     {},
	"extern":   {},
	"false":    {},
	"fn":       {},
	"for":      {},
	"if":       {},
	"impl":     {},
	"in":       {},
	"let":      {},
	"loop":     {},
	"match":    {},
	"mod":      {},
	"move":     {},
	"mut":      {},
	"pub":      {},
	"ref":      {},
	"return":   {},
	"self":     {},
	"Self":     {},
	"static":   {},
	"struct":   {},
	"super":    {},
	"trait":    {},
	"true":     {},
	"type":     {},
	"unsafe":   {},
	"use":      {},
	"where":    {},
	"while":    {},

	// Keywords reserved for future use (future-proofing...)
	"abstract": {},
	"alignof":  {},
	"await":    {},
	"become":   {},
	"do":       {},
	"final":    {},
	"macro":    {},
	"offsetof": {},
	"override": {},
	"priv":     {},
	"proc":     {},
	"pure":     {},
	"sizeof":   {},
	"typeof":   {},
	"unsized":  {},
	"virtual":  {},
	"yield":    {},

	// Weak keywords (special meaning in specific contexts)
	// These are ok in all contexts of FIDL names.
	//"default":	{},
	//"union":	{},

	// Things that are not keywords, but for which collisions would be very
	// unpleasant
	"Result":  {},
	"Ok":      {},
	"Err":     {},
	"Vec":     {},
	"Option":  {},
	"Some":    {},
	"None":    {},
	"Box":     {},
	"Future":  {},
	"Stream":  {},
	"Never":   {},
	"Send":    {},
	"fidl":    {},
	"futures": {},
	"zx":      {},
	"async":   {},
	"on_open": {},
	"OnOpen":  {},
	// TODO(https://fxbug.dev/42145610): Remove "WaitForEvent".
	"wait_for_event": {},
	"WaitForEvent":   {},
}

var reservedSuffixes = []string{
	"Impl",
	"Marker",
	"Proxy",
	"ProxyProtocol",
	"ControlHandle",
	"Responder",
	"Server",
}

func isReservedWord(str string) bool {
	_, ok := reservedWords[str]
	return ok
}

// hasReservedSuffix checks if a string ends with a suffix commonly used by the
// bindings in generated types
func hasReservedSuffix(str string) bool {
	for _, suffix := range reservedSuffixes {
		if strings.HasSuffix(str, suffix) {
			return true
		}
	}
	return false
}

// changeIfReserved adds an underscore suffix to differentiate an identifier
// from a reserved name
//
// Reserved names include a variety of rust keywords, commonly used rust types
// like Result, Vec, and Future, and any name ending in a suffix used by the
// bindings to identify particular generated types like -Impl, -Marker, and
// -Proxy.
func changeIfReserved(val fidlgen.Identifier) string {
	str := string(val)
	if hasReservedSuffix(str) || isReservedWord(str) {
		return str + "_"
	}
	return str
}

var primitiveTypes = map[fidlgen.PrimitiveSubtype]string{
	fidlgen.Bool:    "bool",
	fidlgen.Int8:    "i8",
	fidlgen.Int16:   "i16",
	fidlgen.Int32:   "i32",
	fidlgen.Int64:   "i64",
	fidlgen.Uint8:   "u8",
	fidlgen.Uint16:  "u16",
	fidlgen.Uint32:  "u32",
	fidlgen.Uint64:  "u64",
	fidlgen.Float32: "f32",
	fidlgen.Float64: "f64",
}

var handleSubtypes = map[fidlgen.HandleSubtype]string{
	fidlgen.HandleSubtypeBti:          "fidl::Bti",
	fidlgen.HandleSubtypeChannel:      "fidl::Channel",
	fidlgen.HandleSubtypeClock:        "fidl::Clock",
	fidlgen.HandleSubtypeCounter:      "fidl::Counter",
	fidlgen.HandleSubtypeDebugLog:     "fidl::DebugLog",
	fidlgen.HandleSubtypeEvent:        "fidl::Event",
	fidlgen.HandleSubtypeEventpair:    "fidl::EventPair",
	fidlgen.HandleSubtypeException:    "fidl::Exception",
	fidlgen.HandleSubtypeFifo:         "fidl::Fifo",
	fidlgen.HandleSubtypeGuest:        "fidl::Guest",
	fidlgen.HandleSubtypeInterrupt:    "fidl::Interrupt",
	fidlgen.HandleSubtypeIob:          "fidl::Iob",
	fidlgen.HandleSubtypeIommu:        "fidl::Iommu",
	fidlgen.HandleSubtypeJob:          "fidl::Job",
	fidlgen.HandleSubtypeMsi:          "fidl::Msi",
	fidlgen.HandleSubtypeNone:         "fidl::NullableHandle",
	fidlgen.HandleSubtypePager:        "fidl::Pager",
	fidlgen.HandleSubtypePciDevice:    "fidl::PciDevice",
	fidlgen.HandleSubtypePmt:          "fidl::Pmt",
	fidlgen.HandleSubtypePort:         "fidl::Port",
	fidlgen.HandleSubtypeProcess:      "fidl::Process",
	fidlgen.HandleSubtypeProfile:      "fidl::Profile",
	fidlgen.HandleSubtypeResource:     "fidl::Resource",
	fidlgen.HandleSubtypeSocket:       "fidl::Socket",
	fidlgen.HandleSubtypeStream:       "fidl::Stream",
	fidlgen.HandleSubtypeSuspendToken: "fidl::SuspendToken",
	fidlgen.HandleSubtypeThread:       "fidl::Thread",
	fidlgen.HandleSubtypeTimer:        "fidl::Timer",
	fidlgen.HandleSubtypeVcpu:         "fidl::Vcpu",
	fidlgen.HandleSubtypeVmar:         "fidl::Vmar",
	fidlgen.HandleSubtypeVmo:          "fidl::Vmo",
}

var handleSubtypesFdomain = map[fidlgen.HandleSubtype]string{
	fidlgen.HandleSubtypeBti:          "fdomain_client::Bti",
	fidlgen.HandleSubtypeChannel:      "fdomain_client::Channel",
	fidlgen.HandleSubtypeClock:        "fdomain_client::Clock",
	fidlgen.HandleSubtypeCounter:      "fdomain_client::Counter",
	fidlgen.HandleSubtypeDebugLog:     "fdomain_client::DebugLog",
	fidlgen.HandleSubtypeEvent:        "fdomain_client::Event",
	fidlgen.HandleSubtypeEventpair:    "fdomain_client::EventPair",
	fidlgen.HandleSubtypeException:    "fdomain_client::Exception",
	fidlgen.HandleSubtypeFifo:         "fdomain_client::Fifo",
	fidlgen.HandleSubtypeGuest:        "fdomain_client::Guest",
	fidlgen.HandleSubtypeInterrupt:    "fdomain_client::Interrupt",
	fidlgen.HandleSubtypeIob:          "fdomain_client::Iob",
	fidlgen.HandleSubtypeIommu:        "fdomain_client::Iommu",
	fidlgen.HandleSubtypeJob:          "fdomain_client::Job",
	fidlgen.HandleSubtypeMsi:          "fdomain_client::Msi",
	fidlgen.HandleSubtypeNone:         "fdomain_client::NullableHandle",
	fidlgen.HandleSubtypePager:        "fdomain_client::Pager",
	fidlgen.HandleSubtypePciDevice:    "fdomain_client::PciDevice",
	fidlgen.HandleSubtypePmt:          "fdomain_client::Pmt",
	fidlgen.HandleSubtypePort:         "fdomain_client::Port",
	fidlgen.HandleSubtypeProcess:      "fdomain_client::Process",
	fidlgen.HandleSubtypeProfile:      "fdomain_client::Profile",
	fidlgen.HandleSubtypeResource:     "fdomain_client::Resource",
	fidlgen.HandleSubtypeSocket:       "fdomain_client::Socket",
	fidlgen.HandleSubtypeStream:       "fdomain_client::Stream",
	fidlgen.HandleSubtypeSuspendToken: "fdomain_client::SuspendToken",
	fidlgen.HandleSubtypeThread:       "fdomain_client::Thread",
	fidlgen.HandleSubtypeTimer:        "fdomain_client::Timer",
	fidlgen.HandleSubtypeVcpu:         "fdomain_client::Vcpu",
	fidlgen.HandleSubtypeVmar:         "fdomain_client::Vmar",
	fidlgen.HandleSubtypeVmo:          "fdomain_client::Vmo",
}

var objectTypeConsts = map[fidlgen.HandleSubtype]string{
	fidlgen.HandleSubtypeBti:          "fidl::ObjectType::BTI",
	fidlgen.HandleSubtypeChannel:      "fidl::ObjectType::CHANNEL",
	fidlgen.HandleSubtypeClock:        "fidl::ObjectType::CLOCK",
	fidlgen.HandleSubtypeCounter:      "fidl::ObjectType::COUNTER",
	fidlgen.HandleSubtypeDebugLog:     "fidl::ObjectType::DEBUGLOG",
	fidlgen.HandleSubtypeEvent:        "fidl::ObjectType::EVENT",
	fidlgen.HandleSubtypeEventpair:    "fidl::ObjectType::EVENTPAIR",
	fidlgen.HandleSubtypeException:    "fidl::ObjectType::EXCEPTION",
	fidlgen.HandleSubtypeFifo:         "fidl::ObjectType::FIFO",
	fidlgen.HandleSubtypeGuest:        "fidl::ObjectType::GUEST",
	fidlgen.HandleSubtypeInterrupt:    "fidl::ObjectType::INTERRUPT",
	fidlgen.HandleSubtypeIob:          "fidl::ObjectType::IOB",
	fidlgen.HandleSubtypeIommu:        "fidl::ObjectType::IOMMU",
	fidlgen.HandleSubtypeJob:          "fidl::ObjectType::JOB",
	fidlgen.HandleSubtypeMsi:          "fidl::ObjectType::MSI",
	fidlgen.HandleSubtypeNone:         "fidl::ObjectType::NONE",
	fidlgen.HandleSubtypePager:        "fidl::ObjectType::PAGER",
	fidlgen.HandleSubtypePciDevice:    "fidl::ObjectType::PCI_DEVICE",
	fidlgen.HandleSubtypePmt:          "fidl::ObjectType::PMT",
	fidlgen.HandleSubtypePort:         "fidl::ObjectType::PORT",
	fidlgen.HandleSubtypeProcess:      "fidl::ObjectType::PROCESS",
	fidlgen.HandleSubtypeProfile:      "fidl::ObjectType::PROFILE",
	fidlgen.HandleSubtypeResource:     "fidl::ObjectType::RESOURCE",
	fidlgen.HandleSubtypeSocket:       "fidl::ObjectType::SOCKET",
	fidlgen.HandleSubtypeStream:       "fidl::ObjectType::STREAM",
	fidlgen.HandleSubtypeSuspendToken: "fidl::ObjectType::SUSPEND_TOKEN",
	fidlgen.HandleSubtypeThread:       "fidl::ObjectType::THREAD",
	fidlgen.HandleSubtypeTimer:        "fidl::ObjectType::TIMER",
	fidlgen.HandleSubtypeVcpu:         "fidl::ObjectType::VCPU",
	fidlgen.HandleSubtypeVmar:         "fidl::ObjectType::VMAR",
	fidlgen.HandleSubtypeVmo:          "fidl::ObjectType::VMO",
}

type compiler struct {
	decls             fidlgen.DeclInfoMap
	experiments       fidlgen.Experiments
	library           fidlgen.LibraryIdentifier
	externCrates      map[string]struct{}
	cratePrefix       string
	endpointNamespace string
	handleSubtypes    *map[fidlgen.HandleSubtype]string
	dialect           string
	// Raw structs (including ExternalStructs), needed for
	// flattening parameters and in computeUseFidlStructCopy.
	structs  map[fidlgen.EncodedCompoundIdentifier]fidlgen.Struct
	isCommon bool
}

func (c *compiler) inExternalLibrary(eci fidlgen.EncodedCompoundIdentifier) bool {
	return eci.LibraryName() != c.library.Encode()
}

func (c *compiler) lookupDeclInfo(val fidlgen.EncodedCompoundIdentifier) fidlgen.DeclInfo {
	if info, ok := c.decls[val]; ok {
		return info
	}
	panic(fmt.Sprintf("identifier not in decl map: %s", val))
}

func (c *compiler) compileLibraryName(library fidlgen.LibraryIdentifier) string {
	parts := []string{c.cratePrefix}

	for _, part := range library {
		parts = append(parts, string(part))
	}
	return changeIfReserved(fidlgen.Identifier(strings.Join(parts, "_")))
}

// TODO(https://fxbug.dev/42145610): Escaping reserved words should happen *after*
// converting to CamelCase.
func compileCamelIdentifier(val fidlgen.Identifier) string {
	return fidlgen.ToUpperCamelCase(changeIfReserved(val))
}

// TODO(https://fxbug.dev/42145610): Escaping reserved words should happen *after*
// converting to snake_case.
func compileSnakeIdentifier(val fidlgen.Identifier) string {
	return fidlgen.ToSnakeCase(changeIfReserved(val))
}

// TODO(https://fxbug.dev/42145610): Escaping reserved words should happen *after*
// converting to SCREAMING_SNAKE_CASE.
func compileScreamingSnakeIdentifier(val fidlgen.Identifier) string {
	return fidlgen.ConstNameToAllCapsSnake(changeIfReserved(val))
}

func isZirconIdentifier(ci fidlgen.CompoundIdentifier) bool {
	return len(ci.Library) == 1 && ci.Library[0] == fidlgen.Identifier("zx")
}

var zirconAlias = map[fidlgen.EncodedCompoundIdentifier]string{
	"zx/InstantMono":      "fidl::MonotonicInstant",
	"zx/InstantBoot":      "fidl::BootInstant",
	"zx/InstantMonoTicks": "fidl::MonotonicTicks",
	"zx/InstantBootTicks": "fidl::BootTicks",
}

// We special case references to declarations from //zircon/vdso/zx:zx, since we
// don't generate Rust bindings for that library.
var zirconNames = map[fidlgen.EncodedCompoundIdentifier]string{
	"zx/Rights":                "fidl::Rights",
	"zx/ObjType":               "fidl::ObjectType",
	"zx/CHANNEL_MAX_MSG_BYTES": "zx_types::ZX_CHANNEL_MAX_MSG_BYTES",
}

// compileDeclIdentifier returns a Rust path expression referring to the given
// declaration. It qualifies the crate name if it is external.
func (c *compiler) compileDeclIdentifier(val fidlgen.EncodedCompoundIdentifier) string {
	ci := val.Parse()
	if ci.Member != "" {
		panic(fmt.Sprintf("unexpected member: %s", val))
	}
	if isZirconIdentifier(ci) {
		name, ok := zirconNames[val]
		if !ok {
			panic(fmt.Sprintf("unexpected zircon identifier: %s", val))
		}
		return name
	}
	var name string
	if c.lookupDeclInfo(val).Type == fidlgen.ConstDeclType {
		name = compileScreamingSnakeIdentifier(ci.Name)
	} else {
		name = compileCamelIdentifier(ci.Name)
	}
	// TODO(https://fxbug.dev/42145610): This is incorrect. We're calling changeIfReserved
	// a second time, after compileScreamingSnakeIdentifier or
	// compileCamelIdentifier already did. We should only call it once.
	name = changeIfReserved(fidlgen.Identifier(name))
	if c.inExternalLibrary(val) {
		crate := c.compileLibraryName(ci.Library)
		if c.isCommon {
			crate += "_common"
		}
		c.externCrates[crate] = struct{}{}
		return fmt.Sprintf("%s::%s", crate, name)
	}
	return name
}

// compileMemberIdentifier returns a Rust path expression referring to the given
// declaration member. It qualifies the crate name if it is external.
func (c *compiler) compileMemberIdentifier(val fidlgen.EncodedCompoundIdentifier) string {
	ci := val.Parse()
	if ci.Member == "" {
		panic(fmt.Sprintf("expected a member: %s", val))
	}
	decl := val.DeclName()
	declType := c.lookupDeclInfo(decl).Type
	var member string
	switch declType {
	case fidlgen.BitsDeclType:
		member = compileScreamingSnakeIdentifier(ci.Member)
	case fidlgen.EnumDeclType:
		if isZirconIdentifier(ci) && ci.Name == "ObjType" {
			member = compileScreamingSnakeIdentifier(ci.Member)
		} else {
			member = compileCamelIdentifier(ci.Member)
		}
	default:
		panic(fmt.Sprintf("unexpected decl type: %s", declType))
	}
	return fmt.Sprintf("%s::%s", c.compileDeclIdentifier(decl), member)
}

func (c *compiler) compileLiteral(val fidlgen.Literal, typ fidlgen.Type) string {
	switch val.Kind {
	case fidlgen.StringLiteral:
		var b strings.Builder
		b.WriteRune('"')
		for _, r := range val.Value {
			switch r {
			case '\\':
				b.WriteString(`\\`)
			case '"':
				b.WriteString(`\"`)
			case '\n':
				b.WriteString(`\n`)
			case '\r':
				b.WriteString(`\r`)
			case '\t':
				b.WriteString(`\t`)
			default:
				if unicode.IsPrint(r) {
					b.WriteRune(r)
				} else {
					b.WriteString(fmt.Sprintf(`\u{%x}`, r))
				}
			}
		}
		b.WriteRune('"')
		return b.String()
	case fidlgen.NumericLiteral:
		if typ.Kind == fidlgen.PrimitiveType &&
			(typ.PrimitiveSubtype == fidlgen.Float32 || typ.PrimitiveSubtype == fidlgen.Float64) &&
			!strings.ContainsRune(val.Value, '.') {
			return fmt.Sprintf("%s.0", val.Value)
		}
		if typ.Kind == fidlgen.IdentifierType && c.lookupDeclInfo(typ.Identifier).Type == fidlgen.BitsDeclType {
			if val.Value != "0" {
				panic(fmt.Sprintf("integer literal for bits const must be 0, got %s", val.Value))
			}
			return fmt.Sprintf("%s::empty()", c.compileDeclIdentifier(typ.Identifier))
		}
		return val.Value
	case fidlgen.BoolLiteral:
		return val.Value
	case fidlgen.DefaultLiteral:
		return "::Default::default()"
	default:
		panic(fmt.Sprintf("unknown literal kind: %v", val.Kind))
	}
}

func (c *compiler) compileConstant(val fidlgen.Constant, typ fidlgen.Type) string {
	switch val.Kind {
	case fidlgen.IdentifierConstant:
		declType := c.lookupDeclInfo(val.Identifier.DeclName()).Type
		var expr string
		switch declType {
		case fidlgen.ConstDeclType:
			expr = c.compileDeclIdentifier(val.Identifier)
		case fidlgen.BitsDeclType:
			expr = c.compileMemberIdentifier(val.Identifier)
			if typ.Kind == fidlgen.PrimitiveType {
				expr += ".bits()"
			}
		case fidlgen.EnumDeclType:
			expr = c.compileMemberIdentifier(val.Identifier)
			if typ.Kind == fidlgen.PrimitiveType {
				expr += ".into_primitive()"
			}
		default:
			panic(fmt.Sprintf("unexpected decl type: %s", declType))
		}
		// TODO(https://fxbug.dev/42140924): fidlc allows conversions between primitive
		// types. Ideally the JSON IR would model these conversions explicitly.
		// In the meantime, just assume that a cast is always necessary.
		if typ.Kind == fidlgen.PrimitiveType {
			expr = fmt.Sprintf("%s as %s", expr, compilePrimitiveSubtype(typ.PrimitiveSubtype))
		}
		return expr
	case fidlgen.LiteralConstant:
		return c.compileLiteral(*val.Literal, typ)
	case fidlgen.BinaryOperator:
		if typ.Kind == fidlgen.PrimitiveType {
			return val.Value
		}
		decl := c.compileDeclIdentifier(typ.Identifier)
		// TODO(https://github.com/rust-lang/rust/issues/67441):
		// Use from_bits(%s).unwrap() once the const_option feature is stable.
		return fmt.Sprintf("%s::from_bits_truncate(%s)", decl, val.Value)
	default:
		panic(fmt.Sprintf("unknown constant kind: %s", val.Kind))
	}
}

func (c *compiler) compileConst(val fidlgen.Const) Const {
	r := Const{
		Const: val,
		Name:  c.compileDeclIdentifier(val.Name),
		Value: c.compileConstant(val.Value, val.Type),
	}
	if val.Type.Kind == fidlgen.StringType {
		r.Type = "&str"
	} else {
		r.Type = c.compileType(val.Type, nil).Owned
	}
	return r
}

func compilePrimitiveSubtype(val fidlgen.PrimitiveSubtype) string {
	if t, ok := primitiveTypes[val]; ok {
		return t
	}
	panic(fmt.Sprintf("unknown primitive type: %v", val))
}

func (c *compiler) compileHandleSubtype(val fidlgen.HandleSubtype) string {
	if t, ok := (*c.handleSubtypes)[val]; ok {
		return t
	}
	panic(fmt.Sprintf("unknown handle type: %v", val))
}

func compileObjectTypeConst(val fidlgen.HandleSubtype) string {
	if t, ok := objectTypeConsts[val]; ok {
		return t
	}
	panic(fmt.Sprintf("unknown handle type: %v", val))
}

func (c *compiler) compileType(val fidlgen.Type, maybeAlias *fidlgen.PartialTypeConstructor) Type {
	if maybeAlias != nil {
		name, ok := zirconAlias[maybeAlias.Name]
		if ok {
			return Type{
				Resourceness:     c.decls.LookupResourceness(val),
				Kind:             val.Kind,
				Nullable:         val.Nullable,
				PrimitiveSubtype: val.PrimitiveSubtype,
				Identifier:       fidlgen.EncodedCompoundIdentifier(name),
				FidlTemplate:     name,
				Owned:            name,
				Param:            name,
			}
		}
	}

	t := Type{
		Resourceness:     c.decls.LookupResourceness(val),
		Kind:             val.Kind,
		Nullable:         val.Nullable,
		PrimitiveSubtype: val.PrimitiveSubtype,
		Identifier:       val.Identifier,
	}

	switch val.Kind {
	case fidlgen.PrimitiveType:
		s := compilePrimitiveSubtype(val.PrimitiveSubtype)
		t.FidlTemplate = s
		t.Owned = s
		t.Param = s
	case fidlgen.ArrayType:
		el := c.compileType(*val.ElementType, val.MaybeFromAlias)
		t.ElementType = &el
		t.FidlTemplate = fmt.Sprintf("fidl::encoding::Array<%s, %d>", el.FidlTemplate, *val.ElementCount)
		t.Owned = fmt.Sprintf("[%s; %d]", el.Owned, *val.ElementCount)
		if el.IsResourceType() {
			t.Param = t.Owned
		} else {
			t.Param = "&" + t.Owned
		}
	case fidlgen.VectorType:
		el := c.compileType(*val.ElementType, val.MaybeFromAlias)
		t.ElementType = &el
		if val.ElementCount == nil {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::UnboundedVector<%s>", el.FidlTemplate)
		} else {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::Vector<%s, %d>", el.FidlTemplate, *val.ElementCount)
		}
		t.Owned = fmt.Sprintf("Vec<%s>", el.Owned)
		if el.IsResourceType() {
			t.Param = fmt.Sprintf("Vec<%s>", el.Owned)
		} else {
			t.Param = fmt.Sprintf("&[%s]", el.Owned)
		}
		if val.Nullable {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::Optional<%s>", t.FidlTemplate)
			t.Owned = fmt.Sprintf("Option<%s>", t.Owned)
			t.Param = fmt.Sprintf("Option<%s>", t.Param)
		}
	case fidlgen.StringType:
		if val.ElementCount == nil {
			t.FidlTemplate = "fidl::encoding::UnboundedString"
		} else {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::BoundedString<%d>", *val.ElementCount)
		}
		t.Owned = "String"
		t.Param = "&str"
		if val.Nullable {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::Optional<%s>", t.FidlTemplate)
			t.Owned = fmt.Sprintf("Option<%s>", t.Owned)
			t.Param = fmt.Sprintf("Option<%s>", t.Param)
		}
	case fidlgen.HandleType:
		s := c.compileHandleSubtype(val.HandleSubtype)
		objType := compileObjectTypeConst(val.HandleSubtype)
		t.FidlTemplate = fmt.Sprintf("fidl::encoding::HandleType<%s, { %s.into_raw() }, %d>", s, objType, val.HandleRights)
		t.Owned = s
		t.Param = s
		if val.Nullable {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::Optional<%s>", t.FidlTemplate)
			t.Owned = fmt.Sprintf("Option<%s>", t.Owned)
			t.Param = fmt.Sprintf("Option<%s>", t.Param)
		}
	case fidlgen.EndpointType:
		role_type := ""
		switch val.Role {
		case fidlgen.ClientRole:
			role_type = "ClientEnd"
		case fidlgen.ServerRole:
			role_type = "ServerEnd"
		default:
			panic(fmt.Sprintf("unexpected fidlgen.EndpointRole: %#v", val.Role))
		}
		s := ""
		if val.ProtocolTransport != "Driver" {
			s = fmt.Sprintf("%s::%s<%sMarker>", c.endpointNamespace, role_type, c.compileDeclIdentifier(val.Protocol))
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::Endpoint<%s>", s)
		} else {
			s = fmt.Sprintf("fidl_driver::endpoints::Driver%s<%sMarker>", role_type, c.compileDeclIdentifier(val.Protocol))
			t.FidlTemplate = fmt.Sprintf("fidl_driver::encoding::DriverEndpoint<%sMarker>", c.compileDeclIdentifier(val.Protocol))
		}
		t.Owned = s
		t.Param = s
		if val.Nullable {
			t.FidlTemplate = fmt.Sprintf("fidl::encoding::Optional<%s>", t.FidlTemplate)
			t.Owned = fmt.Sprintf("Option<%s>", t.Owned)
			t.Param = fmt.Sprintf("Option<%s>", t.Param)
		}
	case fidlgen.IdentifierType:
		name := c.compileDeclIdentifier(val.Identifier)
		declInfo := c.lookupDeclInfo(val.Identifier)
		t.DeclType = declInfo.Type
		switch declInfo.Type {
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType:
			t.FidlTemplate = name
			t.Owned = name
			t.Param = name
		case fidlgen.StructDeclType, fidlgen.TableDeclType, fidlgen.UnionDeclType:
			t.FidlTemplate = name
			t.Owned = name
			if t.IsResourceType() {
				t.Param = name
			} else {
				t.Param = "&" + name
			}
			if val.Nullable {
				switch declInfo.Type {
				case fidlgen.StructDeclType:
					t.FidlTemplate = fmt.Sprintf("fidl::encoding::Boxed<%s>", t.FidlTemplate)
				case fidlgen.UnionDeclType:
					t.FidlTemplate = fmt.Sprintf("fidl::encoding::OptionalUnion<%s>", t.FidlTemplate)
				default:
					panic(fmt.Sprintf("unexpected type: %s", declInfo.Type))
				}
				t.Owned = fmt.Sprintf("Option<Box<%s>>", t.Owned)
				if declInfo.IsResourceType() {
					t.Param = fmt.Sprintf("Option<%s>", name)
				} else {
					t.Param = fmt.Sprintf("Option<&%s>", name)
				}
			}
		case fidlgen.ProtocolDeclType:
			s := ""
			if val.ProtocolTransport != "Driver" {
				s = fmt.Sprintf("%s::ClientEnd<%sMarker>", c.endpointNamespace, name)
				t.FidlTemplate = fmt.Sprintf("fidl::encoding::Endpoint<%s>", s)
			} else {
				s = fmt.Sprintf("fidl_driver::endpoints::DriverClientEnd<%sMarker>", name)
				t.FidlTemplate = fmt.Sprintf("fidl_driver::encoding::DriverEndpoint<%sMarker>", name)
			}
			t.Owned = s
			t.Param = s
			if val.Nullable {
				t.FidlTemplate = fmt.Sprintf("fidl::encoding::Optional<%s>", t.FidlTemplate)
				t.Owned = fmt.Sprintf("Option<%s>", t.Owned)
				t.Param = fmt.Sprintf("Option<%s>", t.Param)
			}
		default:
			panic(fmt.Sprintf("unexpected type: %v", declInfo.Type))
		}
	case fidlgen.InternalType:
		switch val.InternalSubtype {
		case fidlgen.FrameworkErr:
			s := "fidl::encoding::FrameworkErr"
			t.FidlTemplate = s
			t.Owned = s
			t.Param = s
		default:
			panic(fmt.Sprintf("unknown internal subtype: %v", val.InternalSubtype))
		}
	default:
		panic(fmt.Sprintf("unknown type kind: %v", val.Kind))
	}

	return t
}

// convertParamToEncodeExpr returns an expression that converts a variable v
// from t.Param to a type implementing fidl::encoding::Encode<t.Fidl>.
//
// TODO(https://fxbug.dev/42073194): Remove this once the transition to the new types is
// complete, since parameter types will be encodable as is.
func convertParamToEncodeExpr(v string, t Type) string {
	switch t.Kind {
	case fidlgen.PrimitiveType, fidlgen.StringType, fidlgen.HandleType, fidlgen.EndpointType:
		return v
	case fidlgen.ArrayType:
		if t.IsResourceType() {
			return "&mut " + v
		}
		return v
	case fidlgen.VectorType:
		if t.IsResourceType() {
			return v + ".as_mut()"
		}
		return v
	case fidlgen.IdentifierType:
		switch t.DeclType {
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType:
			return v
		case fidlgen.StructDeclType, fidlgen.TableDeclType, fidlgen.UnionDeclType:
			if t.IsResourceType() {
				if t.Nullable {
					return v + ".as_mut()"
				}
				return "&mut " + v
			}
			return v
		default:
			panic(fmt.Sprintf("unexpected type: %v", t.DeclType))
		}
	default:
		panic(fmt.Sprintf("unknown type kind: %v", t.Kind))
	}
}

// convertResultToEncodeExpr returns an expression that converts a variable v
// from Result<(p.Params[0].Type, p.Params[1].Type, ...), _> to Result<T, _>
// where p is payloadForType(t) and T implements fidl::encoding::Encode<p.FidlType>.
// If len(p.Params) == 1 then the source type is Result<p.Params[0].Type, _>.
func convertResultToEncodeExpr(v string, t Type, p Payload, dialect string) string {
	switch t.DeclType {
	case fidlgen.StructDeclType:
		var names []string
		var exprs []string
		transform := false
		for _, param := range p.Parameters {
			t := *param.StructMemberType
			expr := convertParamToEncodeExpr(param.Name, t)
			if expr != param.Name {
				transform = true
			}
			if t.IsResourceType() {
				expr = convertMutRefParamToEncodeExpr(param.Name, t, dialect)
			} else {
				expr = "*" + expr
			}
			names = append(names, param.Name)
			exprs = append(exprs, expr)
		}
		if transform {
			return fmt.Sprintf("%s.as_mut().map_err(|e| *e).map(|%s| %s)", v, fmtOneOrTuple(names), fmtTuple(exprs))
		}
		if len(names) == 1 {
			return fmt.Sprintf("%s.map(|%s| (%s,))", v, names[0], names[0])
		}
		return v
	case fidlgen.TableDeclType, fidlgen.UnionDeclType:
		if t.IsResourceType() {
			return v + ".as_mut().map_err(|e| *e)"
		}
		return v
	default:
		panic(fmt.Sprintf("unexpected decl type: %s", t.DeclType))
	}
}

// convertMutRefParamToEncodeExpr returns an expression that converts a variable
// v from &mut t.Param to a type implementing fidl::encoding::Encode<t.Fidl>.
//
// TODO(https://fxbug.dev/42073194): Remove this once the transition to the new types is
// complete. This is only needed for convertResultToEncodeExpr.
func convertMutRefParamToEncodeExpr(v string, t Type, dialect string) string {
	switch t.Kind {
	case fidlgen.IdentifierType:
		switch t.DeclType {
		case fidlgen.StructDeclType, fidlgen.TableDeclType, fidlgen.UnionDeclType:
			if t.Nullable {
				if t.IsResourceType() {
					return v + ".as_mut()"
				}
				return v + ".as_ref()"
			}
		}
	}
	return convertMutRefOwnedToEncodeExpr(v, t, dialect)
}

// convertMutRefOwnedToEncodeExpr returns an expression that converts a variable
// v from &mut t.Owned to a type implementing fidl::encoding::Encode<t.Fidl>.
//
// TODO(https://fxbug.dev/42073194): Remove this once the transition to the new types is
// complete. This is only needed for convertMutRefResultToEncodeExpr.
func convertMutRefOwnedToEncodeExpr(v string, t Type, dialect string) string {
	switch t.Kind {
	case fidlgen.PrimitiveType:
		return "*" + v
	case fidlgen.HandleType, fidlgen.EndpointType:
		if t.Nullable {
			return fmt.Sprintf("%s.as_mut().map(|x| std::mem::replace(x, <<%s as fidl::encoding::ResourceDialect>::Handle as fidl::encoding::HandleFor<%s>>::invalid().into()))", v, dialect, dialect)
		}
		return fmt.Sprintf("std::mem::replace(%s, <<%s as fidl::encoding::ResourceDialect>::Handle as fidl::encoding::HandleFor<%s>>::invalid().into())", v, dialect, dialect)
	case fidlgen.StringType:
		if t.Nullable {
			return v + ".as_deref()"
		}
		return v + ".as_str()"
	case fidlgen.ArrayType:
		if t.IsResourceType() {
			return v
		}
		return "&*" + v
	case fidlgen.VectorType:
		if t.Nullable {
			if t.IsResourceType() {
				return v + ".as_deref_mut()"
			}
			return v + ".as_deref()"
		}
		if t.IsResourceType() {
			return v + ".as_mut_slice()"
		}
		return v + ".as_slice()"
	case fidlgen.IdentifierType:
		switch t.DeclType {
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType:
			return "*" + v
		case fidlgen.StructDeclType, fidlgen.TableDeclType, fidlgen.UnionDeclType:
			if t.Nullable {
				if t.IsResourceType() {
					return v + ".as_deref_mut()"
				}
				return v + ".as_deref()"
			}
			if t.IsResourceType() {
				return v
			}
			return "&*" + v
		default:
			panic(fmt.Sprintf("unexpected type: %v", t.DeclType))
		}
	default:
		panic(fmt.Sprintf("unknown type kind: %v", t.Kind))
	}
}

func (c *compiler) compileAlias(val fidlgen.Alias) Alias {
	return Alias{
		Alias: val,
		Name:  c.compileDeclIdentifier(val.Name),
		Type:  c.compileType(val.Type, val.MaybeFromAlias),
	}
}

func (c *compiler) compileBits(val fidlgen.Bits) Bits {
	e := Bits{
		Bits:           val,
		Name:           c.compileDeclIdentifier(val.Name),
		UnderlyingType: c.compileType(val.Type, nil).Owned,
		Members:        []BitsMember{},
	}
	for _, v := range val.Members {
		e.Members = append(e.Members, BitsMember{
			BitsMember: v,
			Name:       compileScreamingSnakeIdentifier(v.Name),
			Value:      c.compileConstant(v.Value, val.Type),
		})
	}
	return e
}

func (c *compiler) compileEnum(val fidlgen.Enum) Enum {
	e := Enum{
		Enum:           val,
		Name:           c.compileDeclIdentifier(val.Name),
		UnderlyingType: compilePrimitiveSubtype(val.Type),
		Members:        []EnumMember{},
	}
	for _, v := range val.Members {
		e.Members = append(e.Members, EnumMember{
			EnumMember: v,
			Name:       compileCamelIdentifier(v.Name),
			Value:      v.Value.Value,
		})
	}
	e.MinMember = findMinEnumMember(val.Type, e.Members).Name
	return e
}

func findMinEnumMember(typ fidlgen.PrimitiveSubtype, members []EnumMember) EnumMember {
	var res EnumMember
	if typ.IsSigned() {
		min := int64(math.MaxInt64)
		for _, m := range members {
			v, err := strconv.ParseInt(m.Value, 10, 64)
			if err != nil {
				panic(fmt.Sprintf("invalid enum member value: %s", err))
			}
			if v < min {
				min = v
				res = m
			}
		}
	} else {
		min := uint64(math.MaxUint64)
		for _, m := range members {
			v, err := strconv.ParseUint(m.Value, 10, 64)
			if err != nil {
				panic(fmt.Sprintf("invalid enum member value: %s", err))
			}
			if v < min {
				min = v
				res = m
			}
		}
	}
	return res
}

// fmtTuple formats items (types or expressions) as a Rust tuple.
func fmtTuple(items []string) string {
	if len(items) == 0 {
		panic("expected at least one item")
	}
	return fmt.Sprintf("(%s,)", strings.Join(items, ", "))
}

// fmtOneOrTuple is like fmtTuple but does not create 1-tuples.
func fmtOneOrTuple(items []string) string {
	switch len(items) {
	case 0:
		panic("expected at least one item")
	case 1:
		return items[0]
	default:
		return fmt.Sprintf("(%s)", strings.Join(items, ", "))
	}
}

func emptyPayload(fidlType string) Payload {
	return Payload{
		FidlTypeTemplate: fidlType,
		OwnedType:        "()",
		TupleType:        "()",
		EncodeExpr:       "()",
		ConvertToTuple:   func(owned string) string { return owned },
		ConvertToFields:  func(owned string) string { return "" },
	}
}

func (c *compiler) payloadForType(payloadType Type) Payload {
	typeName := c.compileDeclIdentifier(payloadType.Identifier)
	st, ok := c.structs[payloadType.Identifier]

	// Not a struct: table or union payload.
	if !ok {
		paramName := "payload"
		return Payload{
			FidlTypeTemplate: typeName,
			OwnedType:        typeName,
			TupleType:        typeName,
			Parameters: []Parameter{{
				Name:      paramName,
				Type:      payloadType.Param,
				OwnedType: payloadType.Owned,
			}},
			EncodeExpr:      convertParamToEncodeExpr(paramName, payloadType),
			ConvertToTuple:  func(owned string) string { return owned },
			ConvertToFields: func(owned string) string { return fmt.Sprintf("%s: %s,", paramName, owned) },
		}
	}

	// Empty struct with error syntax.
	if len(st.Members) == 0 {
		return emptyPayload("fidl::encoding::EmptyStruct")
	}

	// Struct payload. Flatten it to parameters.
	var parameters []Parameter
	var ownedTypes, encodeExprs []string
	for _, v := range st.Members {
		paramName := compileSnakeIdentifier(v.Name)
		typ := c.compileType(v.Type, v.MaybeFromAlias)
		parameters = append(parameters, Parameter{
			Name:             paramName,
			Type:             typ.Param,
			OwnedType:        typ.Owned,
			StructMemberType: &typ,
		})
		ownedTypes = append(ownedTypes, typ.Owned)
		encodeExprs = append(encodeExprs, convertParamToEncodeExpr(paramName, typ))
	}
	return Payload{
		FidlTypeTemplate: typeName,
		OwnedType:        typeName,
		TupleType:        fmtOneOrTuple(ownedTypes),
		Parameters:       parameters,
		EncodeExpr:       fmtTuple(encodeExprs),
		ConvertToTuple: func(owned string) string {
			var exprs []string
			for _, param := range parameters {
				exprs = append(exprs, fmt.Sprintf("%s.%s", owned, param.Name))
			}
			return fmtOneOrTuple(exprs)
		},
		ConvertToFields: func(owned string) string {
			var b strings.Builder
			for _, param := range parameters {
				b.WriteString(fmt.Sprintf("%s: %s.%s,\n", param.Name, owned, param.Name))
			}
			return b.String()
		},
	}
}

func (c *compiler) compileRequest(m fidlgen.Method) Payload {
	if !m.HasRequest {
		// Not applicable (because the method is an event).
		return Payload{}
	}
	if m.RequestPayload == nil {
		// Empty payload, e.g. the request of `Foo();`.
		return emptyPayload("fidl::encoding::EmptyPayload")
	}
	// Struct, table, or union request payload.
	return c.payloadForType(c.compileType(*m.RequestPayload, nil))
}

func (c *compiler) compileResponse(m fidlgen.Method) Payload {
	if !m.HasResponse {
		// Not applicable (because the method is one-way).
		return Payload{}
	}
	if m.ResponsePayload == nil {
		// Empty payload, e.g. the response of `Foo() -> ();` or `-> Foo();`.
		return emptyPayload("fidl::encoding::EmptyPayload")
	}
	if !m.HasResultUnion() {
		// Plain payload with no flexible/error result union.
		return c.payloadForType(c.compileType(*m.ResponsePayload, nil))
	}

	innerType := c.compileType(*m.ValueType, nil)
	inner := c.payloadForType(innerType)
	var errType Type
	if m.HasError {
		errType = c.compileType(*m.ErrorType, nil)
	}

	var p Payload

	// Set FidlType and OwnedType, which are different for each of the 3 cases.
	if m.HasFrameworkError() && m.HasError {
		p.FidlTypeTemplate = fmt.Sprintf("fidl::encoding::FlexibleResultType<%s, %s>", inner.FidlTypeTemplate, errType.FidlTemplate)
		p.OwnedType = fmt.Sprintf("fidl::encoding::FlexibleResult<%s, %s>", inner.OwnedType, errType.Owned)
	} else if m.HasFrameworkError() {
		p.FidlTypeTemplate = fmt.Sprintf("fidl::encoding::FlexibleType<%s>", inner.FidlTypeTemplate)
		p.OwnedType = fmt.Sprintf("fidl::encoding::Flexible<%s>", inner.OwnedType)
	} else if m.HasError {
		p.FidlTypeTemplate = fmt.Sprintf("fidl::encoding::ResultType<%s, %s>", inner.FidlTypeTemplate, errType.FidlTemplate)
		p.OwnedType = fmt.Sprintf("Result<%s, %s>", inner.OwnedType, errType.Owned)
	} else {
		panic("should have returned earlier")
	}

	// Set all the other fields, where all that matters is m.HasError.
	if !m.HasError {
		p.TupleType = inner.TupleType
		p.Parameters = inner.Parameters
		p.EncodeExpr = inner.EncodeExpr
		p.ConvertToTuple = inner.ConvertToTuple
		p.ConvertToFields = inner.ConvertToFields
	} else {
		paramName := "result"
		p.TupleType = c.compileDeclIdentifier(m.ResponsePayload.Identifier)
		p.TupleTypeAliasRhs = fmt.Sprintf("Result<%s, %s>", inner.TupleType, errType.Owned)
		okParamType := "()"
		if len(inner.Parameters) > 0 {
			var paramTypes []string
			for _, param := range inner.Parameters {
				paramTypes = append(paramTypes, param.Type)
			}
			okParamType = fmtOneOrTuple(paramTypes)
		}
		p.Parameters = []Parameter{{
			Name:      paramName,
			Type:      fmt.Sprintf("Result<%s, %s>", okParamType, errType.Param),
			OwnedType: p.TupleType,
		}}
		p.EncodeExpr = convertResultToEncodeExpr(paramName, innerType, inner, c.dialect)
		p.ConvertToTuple = func(owned string) string {
			return fmt.Sprintf("%s.map(|x| %s)", owned, inner.ConvertToTuple("x"))
		}
		p.ConvertToFields = func(owned string) string {
			return fmt.Sprintf("%s: %s.map(|x| %s),", paramName, owned, inner.ConvertToTuple("x"))
		}
	}

	// For FlexibleType and FlexibleResultType, we need to wrap the value before encoding.
	if m.HasFrameworkError() {
		name, _, _ := strings.Cut(p.OwnedType, "<")
		p.EncodeExpr = fmt.Sprintf("%s::new(%s)", name, p.EncodeExpr)
	}

	return p
}

func (c *compiler) compileProtocol(val fidlgen.Protocol) Protocol {
	name := c.compileDeclIdentifier(val.Name)
	r := Protocol{
		Protocol:         val,
		ECI:              val.Name,
		Marker:           name + "Marker",
		Proxy:            name + "Proxy",
		ProxyInterface:   name + "ProxyInterface",
		SynchronousProxy: name + "SynchronousProxy",
		Request:          name + "Request",
		RequestStream:    name + "RequestStream",
		Event:            name + "Event",
		EventStream:      name + "EventStream",
		ControlHandle:    name + "ControlHandle",
		Ordinals:         fidlgen.ToSnakeCase(name) + "_ordinals",
	}
	if discoverableName := strings.Trim(val.GetProtocolName(), "\""); discoverableName != "" {
		r.Discoverable = true
		// TODO(https://fxbug.dev/42051517): Currently discoverable protocols get
		// PROTOCOL_NAME set equal to DEBUG_NAME, and we change both to use the
		// "fuchsia.foo.Bar" format. We should instead use distinct formats for
		// DEBUG_NAME and PROTOCOL_NAME.
		r.DebugName = discoverableName
	} else {
		// TODO(https://fxbug.dev/42051517): Include the library name in DEBUG_NAME, i.e.
		// "fuchsia.foo/Bar" rather than just "Bar".
		r.DebugName = "(anonymous) " + name
	}

	for _, v := range val.Methods {
		snake_name := compileSnakeIdentifier(v.Name)
		m := Method{
			Method:             v,
			Name:               snake_name,
			UpperSnakeName:     strings.ToUpper(snake_name),
			CamelName:          compileCamelIdentifier(v.Name),
			FullyQualifiedName: fmt.Sprintf("%s.%s", val.Name, v.Name),
			Request:            c.compileRequest(v),
			Response:           c.compileResponse(v),
		}
		if v.HasRequest && v.HasResponse {
			m.Responder = name + m.CamelName + "Responder"
			m.ResponseFut = m.CamelName + "ResponseFut"
		}
		r.Methods = append(r.Methods, m)
	}

	return r
}

func (c *compiler) compileService(val fidlgen.Service) Service {
	r := Service{
		Service:     val,
		Name:        c.compileDeclIdentifier(val.Name),
		Members:     []ServiceMember{},
		ServiceName: val.GetServiceName(),
	}

	for _, v := range val.Members {
		m := ServiceMember{
			ServiceMember: v,
			Name:          string(v.Name),
			CamelName:     compileCamelIdentifier(v.Name),
			SnakeName:     compileSnakeIdentifier(v.Name),
			ProtocolType:  c.compileDeclIdentifier(v.Type.Protocol),
		}
		r.Members = append(r.Members, m)
	}

	return r
}

func (c *compiler) compileStructMember(val fidlgen.StructMember) StructMember {
	return StructMember{
		StructMember: val,
		Type:         c.compileType(val.Type, val.MaybeFromAlias),
		Name:         compileSnakeIdentifier(val.Name),
		OffsetV2:     val.FieldShapeV2.Offset,
		Sensitive:    val.HasAttribute("sensitive"),
	}
}

func (c *compiler) computeUseFidlStructCopyForStruct(st fidlgen.Struct) bool {
	if len(st.Members) == 0 {
		// In Rust, structs containing empty structs do not match the C++ struct layout
		// since empty structs have size 0 in Rust -- even in repr(C).
		return false
	}
	for _, member := range st.Members {
		if !c.computeUseFidlStructCopy(member.Type) {
			return false
		}
	}
	return true
}

func (c *compiler) computeUseFidlStructCopy(typ fidlgen.Type) bool {
	if typ.Nullable {
		return false
	}
	switch typ.Kind {
	case fidlgen.ArrayType:
		return c.computeUseFidlStructCopy(*typ.ElementType)
	case fidlgen.VectorType, fidlgen.StringType, fidlgen.HandleType, fidlgen.EndpointType:
		return false
	case fidlgen.PrimitiveType:
		switch typ.PrimitiveSubtype {
		case fidlgen.Bool, fidlgen.Float32, fidlgen.Float64:
			return false
		}
		return true
	case fidlgen.IdentifierType:
		if c.inExternalLibrary(typ.Identifier) {
			return false
		}
		declType := c.lookupDeclInfo(typ.Identifier).Type
		switch declType {
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType, fidlgen.TableDeclType, fidlgen.UnionDeclType:
			return false
		case fidlgen.StructDeclType:
			st, ok := c.structs[typ.Identifier]
			if !ok {
				panic(fmt.Sprintf("struct not found: %v", typ.Identifier))
			}
			return c.computeUseFidlStructCopyForStruct(st)
		default:
			panic(fmt.Sprintf("unknown declaration type: %v", declType))
		}
	default:
		panic(fmt.Sprintf("unknown type kind: %v", typ.Kind))
	}
}

func (c *compiler) resolveStruct(identifier fidlgen.EncodedCompoundIdentifier) *fidlgen.Struct {
	if c.inExternalLibrary(identifier) {
		// This behavior is matched by computeUseFullStructCopy.
		return nil
	}
	if c.lookupDeclInfo(identifier).Type == fidlgen.StructDeclType {
		st, ok := c.structs[identifier]
		if !ok {
			panic(fmt.Sprintf("struct not found: %v", identifier))
		}
		return &st
	}
	return nil
}

func (c *compiler) compileStruct(val fidlgen.Struct) Struct {
	name := c.compileDeclIdentifier(val.Name)
	r := Struct{
		Struct:           val,
		ECI:              val.Name,
		Name:             name,
		Members:          []StructMember{},
		SizeV2:           val.TypeShapeV2.InlineSize,
		AlignmentV2:      val.TypeShapeV2.Alignment,
		PaddingMarkersV2: val.BuildPaddingMarkers(fidlgen.PaddingConfig{}),
		FlattenedPaddingMarkersV2: val.BuildPaddingMarkers(fidlgen.PaddingConfig{
			FlattenStructs: true,
			FlattenArrays:  true,
			ResolveStruct:  c.resolveStruct,
		}),
		Serializable: val.GetSerializable(),
		Sensitive:    val.HasAttribute("sensitive"),
	}

	for _, v := range val.Members {
		member := c.compileStructMember(v)
		r.Members = append(r.Members, member)
		r.HasPadding = r.HasPadding || (v.FieldShapeV2.Padding != 0)
	}

	r.UseFidlStructCopy = c.computeUseFidlStructCopyForStruct(val)

	return r
}

func (c *compiler) compileUnion(val fidlgen.Union) Union {
	r := Union{
		Union:        val,
		ECI:          val.Name,
		Name:         c.compileDeclIdentifier(val.Name),
		Serializable: val.GetSerializable(),
		Sensitive:    val.HasAttribute("sensitive"),
	}
	for _, v := range val.Members {
		r.Members = append(r.Members, UnionMember{
			UnionMember: v,
			Type:        c.compileType(v.Type, v.MaybeFromAlias),
			Name:        compileCamelIdentifier(v.Name),
			Ordinal:     v.Ordinal,
			Sensitive:   v.HasAttribute("sensitive"),
		})
	}
	return r
}

func (c *compiler) compileTable(table fidlgen.Table) Table {
	r := Table{
		Table:        table,
		ECI:          table.Name,
		Name:         c.compileDeclIdentifier(table.Name),
		Serializable: table.GetSerializable(),
		Sensitive:    table.HasAttribute("sensitive"),
	}
	for _, member := range table.Members {
		r.Members = append(r.Members, TableMember{
			TableMember: member,
			Type:        c.compileType(member.Type, member.MaybeFromAlias),
			Name:        compileSnakeIdentifier(member.Name),
			Ordinal:     member.Ordinal,
			Sensitive:   member.HasAttribute("sensitive"),
		})
	}
	return r
}

type derives uint16

const (
	derivesDebug derives = 1 << iota
	derivesCopy
	derivesClone
	derivesDefault
	derivesEq
	derivesPartialEq
	derivesOrd
	derivesPartialOrd
	derivesHash
	derivesAll derives = (1 << iota) - 1
)

// note: keep this list in the same order as the derives definitions
var derivesNames = []string{
	// [START derived_traits]
	"Debug",
	"Copy",
	"Clone",
	"Default",
	"Eq",
	"PartialEq",
	"Ord",
	"PartialOrd",
	"Hash",
	// [END derived_traits]
}

// Returns the derives that are allowed for a type based on resourceness
func allowedDerives(r fidlgen.Resourceness) derives {
	if r.IsResourceType() {
		return derivesAll &^ (derivesCopy | derivesClone)
	}
	return derivesAll
}

// Returns the minimal derives we can assume a type has based on resourceness.
func minimalDerives(r fidlgen.Resourceness) derives {
	if r.IsValueType() {
		return derivesDebug | derivesPartialEq | derivesClone
	}
	return derivesDebug | derivesPartialEq
}

// RemoveCustom removes traits by name. Templates use it when they are providing
// a custom impl instead of deriving. (We can't just omit the traits in
// fillDerives because it would recursively remove it from containing types.)
func (v derives) RemoveCustom(traits ...string) (derives, error) {
	result := v
	for _, name := range traits {
		found := false
		for i, n := range derivesNames {
			if n == name {
				found = true
				result = result &^ (1 << i)
				break
			}
		}
		if !found {
			return 0, fmt.Errorf("trait '%s' not found", name)
		}
	}
	return result, nil
}

func (v derives) String() string {
	var parts []string
	for i, bit := 0, derives(1); bit&derivesAll != 0; i, bit = i+1, bit<<1 {
		if v&bit != 0 {
			parts = append(parts, derivesNames[i])
		}
	}
	if len(parts) == 0 {
		return ""
	}
	sort.Strings(parts)
	return fmt.Sprintf("#[derive(%s)]", strings.Join(parts, ", "))
}

// The status of derive calculation for a particular type.
type deriveStatus struct {
	// recursing indicates whether or not we've passed through this type already
	// on a recursive descent. This is used to prevent unbounded recursion on
	// mutually-recursive types.
	recursing bool
	// complete indicates whether or not the derive for the given type has
	// already been successfully calculated and stored in the IR.
	complete bool
}

// The state of the current calculation of derives.
type derivesCompiler struct {
	*compiler
	topMostCall                bool
	didShortCircuitOnRecursion bool
	statuses                   map[EncodedCompoundIdentifier]deriveStatus
	root                       *Root
}

// [START fill_derives]
// Calculates what traits should be derived for each output type,
// filling in all `*derives` in the IR.
func (c *compiler) fillDerives(ir *Root) {
	// [END fill_derives]
	dc := &derivesCompiler{
		compiler:                   c,
		topMostCall:                true,
		didShortCircuitOnRecursion: false,
		statuses:                   make(map[EncodedCompoundIdentifier]deriveStatus),
		root:                       ir,
	}

	// Bits derives are controlled by the bitflags crate.
	// Enums derive a constant set of traits hardcoded in enum.tmpl.
	for _, v := range ir.Structs {
		dc.fillDerivesForECI(v.ECI)
	}
	for _, v := range ir.Unions {
		dc.fillDerivesForECI(v.ECI)
	}
	for _, v := range ir.Tables {
		dc.fillDerivesForECI(v.ECI)
	}
}

// Computes derives for a struct, table, or union.
// Also fills in the .Derives field if the type is local to this library.
func (dc *derivesCompiler) fillDerivesForECI(eci EncodedCompoundIdentifier) derives {
	declInfo := dc.lookupDeclInfo(eci)

	// TODO(https://fxbug.dev/42140082): Make external type information available here.
	// Currently, we conservatively assume external structs, tables, and unions
	// only derive a minimal set of traits, which includes Clone for value types
	// (not having Clone is especially annoying, so we put resourceness of
	// external types into the IR as a stopgap solution).
	if dc.inExternalLibrary(eci) {
		return minimalDerives(*declInfo.Resourceness)
	}

	topMostCall := dc.topMostCall
	if dc.topMostCall {
		dc.topMostCall = false
	}
	deriveStatus := dc.statuses[eci]
	if deriveStatus.recursing {
		// If we've already seen the current type while recursing,
		// the algorithm has already explored all of the other fields contained
		// within the cycle, so we can return true for all derives, and the
		// correct results will be bubbled up.
		dc.didShortCircuitOnRecursion = true
		return derivesAll
	}
	deriveStatus.recursing = true
	dc.statuses[eci] = deriveStatus

	var derivesOut derives
	switch declInfo.Type {
	case fidlgen.StructDeclType:
		st := dc.root.findStruct(eci)
		if st == nil {
			panic(fmt.Sprintf("struct not found: %v", eci))
		}
		if deriveStatus.complete {
			derivesOut = st.Derives
			break
		}
		derivesOut = allowedDerives(st.Resourceness)
		for _, member := range st.Members {
			derivesOut &= dc.derivesForType(member.Type)
		}
		derivesOut &^= derivesDefault
		st.Derives = derivesOut
	case fidlgen.TableDeclType:
		table := dc.root.findTable(eci)
		if table == nil {
			panic(fmt.Sprintf("table not found: %v", eci))
		}
		if deriveStatus.complete {
			derivesOut = table.Derives
			break
		}
		derivesOut = minimalDerives(*declInfo.Resourceness)
		// We can always derive Default because table fields are optional.
		derivesOut |= derivesDefault
		table.Derives = derivesOut
	case fidlgen.UnionDeclType:
		union := dc.root.findUnion(eci)
		if union == nil {
			panic(fmt.Sprintf("union not found: %v", eci))
		}
		if deriveStatus.complete {
			derivesOut = union.Derives
			break
		}
		if union.IsFlexible() {
			derivesOut = minimalDerives(union.Resourceness)
		} else {
			derivesOut = allowedDerives(union.Resourceness)
			for _, member := range union.Members {
				derivesOut &= dc.derivesForType(member.Type)
			}
		}
		derivesOut &^= derivesDefault
		union.Derives = derivesOut
	default:
		panic(fmt.Sprintf("unexpected declaration type: %v", declInfo.Type))
	}
	if topMostCall || !dc.didShortCircuitOnRecursion {
		// Our completed result is only valid if it's either at top-level
		// (ensuring we've fully visited all child types in the recursive
		// substructure at least once) or if we performed no recursion-based
		// short-circuiting, in which case results are correct and absolute.
		//
		// Note that non-topMostCalls are invalid if we did short-circuit
		// on recursion, because we might have short-circuited just beneath
		// a type without having explored all of its children at least once
		// beneath it.
		//
		// For example, imagine A -> B -> C -> A.
		// When we start on A, we go to B, then go to C, then go to A, at which
		// point we short-circuit. The intermediate result for C is invalid
		// for anything except the computation of A and B above, as it does
		// not take into account that C contains A and B, only that it contains
		// its top-level fields (other than A). In order to get a correct
		// idea of the shape of C, we have to start with C, following through
		// every branch until we find C again.
		deriveStatus.complete = true
	}
	if topMostCall {
		// Reset intermediate state
		dc.topMostCall = true
		dc.didShortCircuitOnRecursion = false
	}
	deriveStatus.recursing = false
	dc.statuses[eci] = deriveStatus
	return derivesOut
}

func (dc *derivesCompiler) derivesForType(t Type) derives {
	switch t.Kind {
	case fidlgen.ArrayType:
		return dc.derivesForType(*t.ElementType)
	case fidlgen.VectorType:
		return derivesAll &^ derivesCopy & dc.derivesForType(*t.ElementType)
	case fidlgen.StringType:
		return derivesAll &^ derivesCopy
	case fidlgen.HandleType, fidlgen.EndpointType:
		return derivesAll &^ (derivesCopy | derivesClone)
	case fidlgen.PrimitiveType:
		switch t.PrimitiveSubtype {
		case fidlgen.Bool:
			return derivesAll
		case fidlgen.Int8, fidlgen.Int16, fidlgen.Int32, fidlgen.Int64,
			fidlgen.Uint8, fidlgen.Uint16, fidlgen.Uint32, fidlgen.Uint64:
			return derivesAll
		case fidlgen.Float32, fidlgen.Float64:
			// Floats don't have a total ordering due to NAN and its multiple representations.
			return derivesAll &^ (derivesEq | derivesOrd | derivesHash)
		default:
			panic(fmt.Sprintf("unknown primitive type: %v", t.PrimitiveSubtype))
		}
	case fidlgen.IdentifierType:
		switch t.DeclType {
		case fidlgen.BitsDeclType, fidlgen.EnumDeclType:
			return derivesAll
		case fidlgen.StructDeclType, fidlgen.UnionDeclType:
			result := dc.fillDerivesForECI(t.Identifier)
			if t.Nullable {
				// Nullable structs and unions gets put in Option<Box<...>>.
				result &^= derivesCopy
			}
			return result
		case fidlgen.TableDeclType:
			return dc.fillDerivesForECI(t.Identifier)
		default:
			panic(fmt.Sprintf("unexpected identifier type: %v", t.DeclType))
		}
	default:
		panic(fmt.Sprintf("unknown type kind: %v", t.Kind))
	}
}

func Compile(r fidlgen.Root, includeDrivers bool, fdomain bool, isCommon bool) Root {
	r = r.ForBindings("rust")
	transports := []string{"Channel"}
	if includeDrivers {
		transports = append(transports, "Driver")
	}
	r = r.ForTransports(transports)

	cratePrefix := "fidl"
	endpointNamespace := "fidl::endpoints"
	handleSubtypes := &handleSubtypes
	dialect := "fidl::encoding::DefaultFuchsiaResourceDialect"

	if fdomain {
		cratePrefix = "fdomain"
		endpointNamespace = "fdomain_client::fidl"
		handleSubtypes = &handleSubtypesFdomain
		dialect = "fdomain_client::fidl::FDomainResourceDialect"
	}

	root := Root{
		Experiments: r.Experiments,
	}
	thisLibParsed := r.Name.Parse()
	c := compiler{
		decls:             r.DeclInfo(),
		experiments:       r.Experiments,
		library:           thisLibParsed,
		cratePrefix:       cratePrefix,
		endpointNamespace: endpointNamespace,
		dialect:           dialect,
		handleSubtypes:    handleSubtypes,
		externCrates:      map[string]struct{}{},
		structs:           map[fidlgen.EncodedCompoundIdentifier]fidlgen.Struct{},
		isCommon:          isCommon,
	}
	for _, s := range r.Structs {
		c.structs[s.Name] = s
	}

	for _, s := range r.ExternalStructs {
		c.structs[s.Name] = s
	}

	for _, v := range r.Aliases {
		root.Aliases = append(root.Aliases, c.compileAlias(v))
	}

	for _, v := range r.Bits {
		root.Bits = append(root.Bits, c.compileBits(v))
	}

	for _, v := range r.Consts {
		root.Consts = append(root.Consts, c.compileConst(v))
	}

	for _, v := range r.Enums {
		root.Enums = append(root.Enums, c.compileEnum(v))
	}

	for _, v := range r.Services {
		root.Services = append(root.Services, c.compileService(v))
	}

	for _, v := range r.Tables {
		root.Tables = append(root.Tables, c.compileTable(v))
	}

	for _, v := range r.Structs {
		if v.IsEmptySuccessStruct {
			continue
		}
		root.Structs = append(root.Structs, c.compileStruct(v))
	}

	for _, v := range r.Unions {
		if v.IsResult {
			continue
		}
		root.Unions = append(root.Unions, c.compileUnion(v))
	}

	for _, v := range r.Protocols {
		root.Protocols = append(root.Protocols, c.compileProtocol(v))
	}

	c.fillDerives(&root)

	thisLibCompiled := c.compileLibraryName(thisLibParsed)

	// Sort the extern crates to make sure the generated file is
	// consistent across builds.
	var externCrates []string
	for k := range c.externCrates {
		externCrates = append(externCrates, k)
	}
	sort.Strings(externCrates)

	for _, k := range externCrates {
		if k != thisLibCompiled {
			root.ExternCrates = append(root.ExternCrates, k)
		}
	}

	return root
}

func (s Struct) HasSensitive() bool {
	if s.Sensitive {
		return true
	}
	for _, member := range s.Members {
		if member.Sensitive {
			return true
		}
	}
	return false
}

func (t Table) HasSensitive() bool {
	if t.Sensitive {
		return true
	}
	for _, member := range t.Members {
		if member.Sensitive {
			return true
		}
	}
	return false
}

func (u Union) HasSensitive() bool {
	if u.Sensitive {
		return true
	}
	for _, member := range u.Members {
		if member.Sensitive {
			return true
		}
	}
	return false
}
