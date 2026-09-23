// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package cpp

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/util"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type handleRepr int

const (
	_ = iota
	handleReprDisposition
	handleReprInfo
	HandleReprRaw
)

func BuildValue(value ir.Value, decl mixer.Declaration, handleRepr handleRepr) (string, string) {
	builder := builder{
		handleRepr: handleRepr,
	}
	valueVar := builder.visit(value, decl)
	valueBuild := builder.String()
	return valueBuild, valueVar
}

type builder struct {
	strings.Builder
	varidx     int
	handleRepr handleRepr
}

func (b *builder) write(format string, vals ...any) {
	b.WriteString(fmt.Sprintf(format, vals...))
}

func (b *builder) newVar() string {
	b.varidx++
	return fmt.Sprintf("var%d", b.varidx)
}

func (b *builder) assignNew(typename string, isPointer bool, fmtStr string, vals ...any) string {
	rhs := b.construct(typename, isPointer, fmtStr, vals...)
	newVar := b.newVar()
	b.write("auto %s = %s;\n", newVar, rhs)
	return newVar
}

func (b *builder) construct(typename string, isPointer bool, fmtStr string, args ...any) string {
	val := fmt.Sprintf(fmtStr, args...)
	if !isPointer {
		return fmt.Sprintf("%s(%s)", typename, val)
	}
	return fmt.Sprintf("std::make_unique<%s>(%s)", typename, val)
}

func (b *builder) adoptHandle(decl mixer.Declaration, value ir.Handle) string {
	if b.handleRepr == handleReprDisposition || b.handleRepr == handleReprInfo {
		return fmt.Sprintf("%s(handle_defs[%d].handle)", TypeName(decl), value)
	}
	return fmt.Sprintf("%s(handle_defs[%d])", TypeName(decl), value)
}

func FormatPrimitive(value ir.Value) string {
	switch value := value.(type) {
	case int64:
		if value == -9223372036854775808 {
			return "-9223372036854775807ll - 1"
		}
		return fmt.Sprintf("%dll", value)
	case uint64:
		return fmt.Sprintf("%dull", value)
	case float64:
		return fmt.Sprintf("%g", value)
	}
	panic(fmt.Sprintf("unknown primitive type %T", value))
}

func (a *builder) visit(value ir.Value, decl mixer.Declaration) string {
	// std::optional is used to represent nullability for strings and vectors.
	_, isString := decl.(*mixer.StringDecl)
	_, isVector := decl.(*mixer.VectorDecl)
	isPointer := (decl.IsNullable() && !isString && !isVector)

	switch value := value.(type) {
	case bool:
		return a.construct(TypeName(decl), isPointer, "%t", value)
	case int64, uint64, float64:
		switch decl := decl.(type) {
		case mixer.PrimitiveDeclaration, *mixer.EnumDecl:
			return a.construct(TypeName(decl), isPointer, FormatPrimitive(value))
		case *mixer.BitsDecl:
			return fmt.Sprintf("static_cast<%s>(%s)", DeclName(decl), FormatPrimitive(value))
		}
	case ir.RawFloat:
		switch decl.(*mixer.FloatDecl).Subtype() {
		case fidlgen.Float32:
			return fmt.Sprintf("([] { uint32_t u = %#b; float f; memcpy(&f, &u, sizeof(float)); return f; })()", value)
		case fidlgen.Float64:
			return fmt.Sprintf("([] { uint64_t u = %#b; double d; memcpy(&d, &u, sizeof(double)); return d; })()", value)
		}
	case string:
		return a.construct(typeNameIgnoreNullable(decl), isPointer, "%q, %d", value, len(value))
	case ir.Handle:
		switch decl := decl.(type) {
		case *mixer.HandleDecl:
			return a.adoptHandle(decl, value)
		case *mixer.ClientEndDecl:
			return fmt.Sprintf("%s(%s)", TypeName(decl), a.adoptHandle(decl.UnderlyingHandleDecl(), value))
		case *mixer.ServerEndDecl:
			return fmt.Sprintf("%s(%s)", TypeName(decl), a.adoptHandle(decl.UnderlyingHandleDecl(), value))
		}
	case ir.Record:
		switch decl := decl.(type) {
		case *mixer.StructDecl:
			return a.visitStructOrTable(value, decl, isPointer)
		case *mixer.TableDecl:
			return a.visitStructOrTable(value, decl, isPointer)
		case *mixer.UnionDecl:
			return a.visitUnion(value, decl, isPointer)
		}
	case []ir.Value:
		switch decl := decl.(type) {
		case *mixer.ArrayDecl:
			return a.visitArray(value, decl, isPointer)
		case *mixer.VectorDecl:
			return a.visitVector(value, decl, isPointer)
		}
	case nil:
		return a.construct(TypeName(decl), false, "")
	}
	panic(fmt.Sprintf("not implemented: %T", value))
}

func (b *builder) visitStructOrTable(value ir.Record, decl mixer.RecordDeclaration, isPointer bool) string {
	s := b.newVar()
	structRaw := fmt.Sprintf("%s{::fidl::internal::DefaultConstructPossiblyInvalidObjectTag{}}", typeNameIgnoreNullable(decl))
	var op string
	if isPointer {
		op = "->"
		b.write("auto %s = fidl::Box(std::make_unique<%s>(%s));\n", s, typeNameIgnoreNullable(decl), structRaw)
	} else {
		op = "."
		b.write("auto %s = %s;\n", s, structRaw)
	}

	for _, field := range value.Fields {
		fieldRhs := b.visit(field.Value, decl.Field(field.Key.Name))
		b.write("%s%s%s() = %s;\n", s, op, field.Key.Name, fieldRhs)
	}

	return fmt.Sprintf("std::move(%s)", s)
}

func (a *builder) visitUnion(value ir.Record, decl *mixer.UnionDecl, isPointer bool) string {
	if len(value.Fields) == 0 {
		return a.assignNew(typeNameIgnoreNullable(decl), isPointer, "")
	}

	field := value.Fields[0]
	if field.Key.IsUnknown() {
		panic(fmt.Sprintf("union %s: unknown ordinal %d: C++ cannot construct unions with unknown fields",
			decl.Name(), field.Key.UnknownOrdinal))
	}
	fieldRhs := a.visit(field.Value, decl.Field(field.Key.Name))

	varName := a.assignNew(typeNameIgnoreNullable(decl), isPointer, "%s::With%s(%s)", typeNameIgnoreNullable(decl), fidlgen.ToUpperCamelCase(field.Key.Name), fieldRhs)
	return fmt.Sprintf("std::move(%s)", varName)
}

func (a *builder) visitArray(value []ir.Value, decl *mixer.ArrayDecl, isPointer bool) string {
	array := a.assignNew(typeNameIgnoreNullable(decl), isPointer, "::fidl::internal::DefaultConstructPossiblyInvalidObject<%s>::Make()", typeNameIgnoreNullable(decl))
	op := ""
	if isPointer {
		op = ".get()"
	}
	for i, item := range value {
		elem := a.visit(item, decl.Elem())
		a.write("%s%s[%d] = %s;\n", array, op, i, elem)
	}
	return fmt.Sprintf("std::move(%s)", array)
}

func (a *builder) visitVector(value []ir.Value, decl *mixer.VectorDecl, isPointer bool) string {
	vector := a.assignNew(TypeName(decl), isPointer, "")
	if decl.IsNullable() {
		a.write("%s.emplace();\n", vector)
	}
	if len(value) == 0 {
		return fmt.Sprintf("std::move(%s)", vector)
	}
	// Special case unsigned integer vectors, because clang otherwise has issues with large vectors on arm.
	// This uses pattern matching so only a subset of byte vectors that fit the pattern (repeating sequence) will be optimized.
	if elemDecl, ok := decl.Elem().(mixer.PrimitiveDeclaration); ok && elemDecl.Subtype() == fidlgen.Uint8 {
		var uintValues []uint64
		for _, v := range value {
			uintValues = append(uintValues, v.(uint64))
		}
		// For simplicity, only support sizes that are multiples of the period.
		if period, ok := util.FindRepeatingPeriod(uintValues); ok && len(value)%period == 0 {
			if decl.IsNullable() {
				a.write("%s.value().resize(%d);\n", vector, len(value))
			} else {
				a.write("%s.resize(%d);\n", vector, len(value))
			}
			for i := 0; i < period; i++ {
				elem := a.visit(value[i], decl.Elem())
				a.write("%s[%d] = %s;\n", vector, i, elem)
			}
			a.write(
				`for (size_t offset = 0; offset < %[1]s.size(); offset += %[2]d) {
memcpy(%[1]s.data() + offset, %[1]s.data(), %[2]d);
}
`, vector, period)
			return fmt.Sprintf("std::move(%s)", vector)
		}
		if len(uintValues) > 1024 {
			panic("large vectors that are not repeating are not supported, for build performance reasons")
		}
	}

	for _, item := range value {
		elem := a.visit(item, decl.Elem())
		if decl.IsNullable() {
			a.write("%s.value().emplace_back(%s);\n", vector, elem)
		} else {
			a.write("%s.emplace_back(%s);\n", vector, elem)
		}
	}
	return fmt.Sprintf("std::move(%s)", vector)
}

func typeNameImpl(decl mixer.Declaration, ignoreNullable bool) string {
	switch decl := decl.(type) {
	case mixer.PrimitiveDeclaration:
		return primitiveTypeName(decl.Subtype())
	case *mixer.StringDecl:
		if !ignoreNullable && decl.IsNullable() {
			return "std::optional<std::string>"
		}
		return "std::string"
	case *mixer.StructDecl:
		if !ignoreNullable && decl.IsNullable() {
			return fmt.Sprintf("fidl::Box<%s>", DeclName(decl))
		}
		return DeclName(decl)
	case *mixer.UnionDecl:
		if !ignoreNullable && decl.IsNullable() {
			return fmt.Sprintf("fidl::Box<%s>", DeclName(decl))
		}
		return DeclName(decl)
	case *mixer.ArrayDecl:
		return fmt.Sprintf("std::array<%s, %d>", TypeName(decl.Elem()), decl.Size())
	case *mixer.VectorDecl:
		if !ignoreNullable && decl.IsNullable() {
			return fmt.Sprintf("std::optional<std::vector<%s>>", TypeName(decl.Elem()))
		}
		return fmt.Sprintf("std::vector<%s>", TypeName(decl.Elem()))
	case *mixer.HandleDecl:
		switch decl.Subtype() {
		case fidlgen.HandleSubtypeNone:
			return "zx::handle"
		case fidlgen.HandleSubtypeChannel:
			return "zx::channel"
		case fidlgen.HandleSubtypeEvent:
			return "zx::event"
		default:
			panic(fmt.Sprintf("Handle subtype not supported %s", decl.Subtype()))
		}
	case *mixer.ClientEndDecl:
		return fmt.Sprintf("fidl::ClientEnd<%s>", EndpointDeclName(decl))
	case *mixer.ServerEndDecl:
		return fmt.Sprintf("fidl::ServerEnd<%s>", EndpointDeclName(decl))
	case mixer.NamedDeclaration:
		return DeclName(decl)
	default:
		panic("unhandled case")
	}
}

func TypeName(decl mixer.Declaration) string {
	return typeNameImpl(decl, false)
}

func typeNameIgnoreNullable(decl mixer.Declaration) string {
	return typeNameImpl(decl, true)
}

func DeclName(decl mixer.NamedDeclaration) string {
	parts := strings.SplitN(decl.Name(), "/", 2)
	return fmt.Sprintf("%s::%s", strings.ReplaceAll(parts[0], ".", "_"), fidlgen.ToUpperCamelCase(parts[1]))
}

func EndpointDeclName(decl mixer.EndpointDeclaration) string {
	parts := strings.SplitN(decl.ProtocolName(), "/", 2)
	return fmt.Sprintf("%s::%s", strings.ReplaceAll(parts[0], ".", "_"), fidlgen.ToUpperCamelCase(parts[1]))
}

func primitiveTypeName(subtype fidlgen.PrimitiveSubtype) string {
	switch subtype {
	case fidlgen.Bool:
		return "bool"
	case fidlgen.Uint8, fidlgen.Uint16, fidlgen.Uint32, fidlgen.Uint64,
		fidlgen.Int8, fidlgen.Int16, fidlgen.Int32, fidlgen.Int64:
		return fmt.Sprintf("%s_t", subtype)
	case fidlgen.Float32:
		return "float"
	case fidlgen.Float64:
		return "double"
	default:
		panic(fmt.Sprintf("unexpected subtype %s", subtype))
	}
}
