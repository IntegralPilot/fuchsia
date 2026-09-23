// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust

import (
	"fmt"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/rust"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

func buildHandleDefs(defs []ir.HandleDef) string {
	if len(defs) == 0 {
		return ""
	}
	var builder strings.Builder
	builder.WriteString("[\n")
	for i, d := range defs {
		// Write indices corresponding to the .gidl file handle_defs block.
		builder.WriteString(fmt.Sprintf(
			`// #%d
HandleDef{
	subtype: HandleSubtype::%s,
	rights: Rights::from_bits(%d).unwrap(),
},
`, i, handleTypeName(d.Subtype), d.Rights))
	}
	builder.WriteString("]")
	return builder.String()
}

func buildHandles(handles []ir.Handle) string {
	var builder strings.Builder
	builder.WriteString("[\n")
	for i, h := range handles {
		builder.WriteString(fmt.Sprintf("%d,", h))
		if i%8 == 7 {
			builder.WriteString("\n")
		}
	}
	builder.WriteString("]")
	return builder.String()
}

func buildRawHandleDispositions(handleDispositions []ir.HandleDisposition) string {
	var builder strings.Builder
	builder.WriteString("[")
	for _, hd := range handleDispositions {
		fmt.Fprintf(&builder, `
zx_types::zx_handle_disposition_t {
   operation: zx_types::ZX_HANDLE_OP_MOVE,
   handle: handle_defs[%d].handle,
   type_: %d,
   rights: %d,
   result: zx_types::ZX_OK,
},`, hd.Handle, hd.Type, hd.Rights)
	}
	builder.WriteString("]")
	return builder.String()
}

func buildRawHandles(handleDispositions []ir.HandleDisposition) string {
	var builder strings.Builder
	builder.WriteString("[")
	for _, hd := range handleDispositions {
		fmt.Fprintf(&builder, "handle_defs[%d].handle,\n", hd.Handle)
	}
	builder.WriteString("]")
	return builder.String()
}

func visit(value ir.Value, decl mixer.Declaration) string {
	switch value := value.(type) {
	case bool:
		return strconv.FormatBool(value)
	case int64, uint64, float64:
		switch decl := decl.(type) {
		case mixer.PrimitiveDeclaration:
			suffix := primitiveTypeName(decl.Subtype())
			return fmt.Sprintf("%v%s", value, suffix)
		case *mixer.BitsDecl:
			primitive := visit(value, &decl.Underlying)
			if decl.IsFlexible() {
				return fmt.Sprintf("%s::from_bits_allow_unknown(%v)", declName(decl), primitive)
			}
			// Use from_bits_retain so that encode_failure tests work. It's
			// not worth the effort to make the test type available here and use
			// from_bits(...).unwrap() in success cases, since all this would do
			// is move validation from the bindings to GIDL.
			return fmt.Sprintf("%s::from_bits_retain(%v)", declName(decl), primitive)
		case *mixer.EnumDecl:
			primitive := visit(value, &decl.Underlying)
			if decl.IsFlexible() {
				return fmt.Sprintf("%s::from_primitive_allow_unknown(%v)", declName(decl), primitive)
			}
			return fmt.Sprintf("%s::from_primitive(%v).unwrap()", declName(decl), primitive)
		}
	case ir.RawFloat:
		switch decl.(*mixer.FloatDecl).Subtype() {
		case fidlgen.Float32:
			return fmt.Sprintf("f32::from_bits(%#b)", value)
		case fidlgen.Float64:
			return fmt.Sprintf("f64::from_bits(%#b)", value)
		}
	case string:
		var expr string
		if fidlgen.PrintableASCII(value) {
			expr = fmt.Sprintf("String::from(%q)", value)
		} else {
			expr = fmt.Sprintf("std::str::from_utf8(b\"%s\").unwrap().to_string()", rust.EscapeStr(value))
		}
		return wrapNullable(decl, expr)
	case ir.Handle:
		expr := buildHandleValue(value)
		return wrapNullable(decl, expr)
	case ir.Record:
		switch decl := decl.(type) {
		case *mixer.StructDecl:
			return onStruct(value, decl)
		case *mixer.TableDecl:
			return onTable(value, decl)
		case *mixer.UnionDecl:
			return onUnion(value, decl)
		}
	case []ir.Value:
		switch decl := decl.(type) {
		case *mixer.ArrayDecl:
			return onList(value, decl)
		case *mixer.VectorDecl:
			return onList(value, decl)
		}
	case ir.DecodedRecord:
		return onDecodedRecord(value, decl.(mixer.RecordDeclaration))
	case nil:
		if !decl.IsNullable() {
			if _, ok := decl.(*mixer.HandleDecl); ok {
				return "NullableHandle::invalid()"
			}
			panic(fmt.Sprintf("got nil for non-nullable type: %T", decl))
		}
		return "None"
	}
	panic(fmt.Sprintf("not implemented: %T", value))
}

func declName(decl mixer.NamedDeclaration) string {
	return identifierName(decl.Name())
}

// TODO(https://fxbug.dev/42115264): Move into a common library outside GIDL.
func identifierName(qualifiedName string) string {
	parts := strings.Split(qualifiedName, "/")
	library_parts := strings.Split(parts[0], ".")
	return fmt.Sprintf("%s::%s", strings.Join(library_parts, "_"),
		fidlgen.ToUpperCamelCase(parts[1]))
}

func primitiveTypeName(subtype fidlgen.PrimitiveSubtype) string {
	switch subtype {
	case fidlgen.Bool:
		return "bool"
	case fidlgen.Int8:
		return "i8"
	case fidlgen.Uint8:
		return "u8"
	case fidlgen.Int16:
		return "i16"
	case fidlgen.Uint16:
		return "u16"
	case fidlgen.Int32:
		return "i32"
	case fidlgen.Uint32:
		return "u32"
	case fidlgen.Int64:
		return "i64"
	case fidlgen.Uint64:
		return "u64"
	case fidlgen.Float32:
		return "f32"
	case fidlgen.Float64:
		return "f64"
	default:
		panic(fmt.Sprintf("unexpected subtype %v", subtype))
	}
}

func handleTypeName(subtype fidlgen.HandleSubtype) string {
	switch subtype {
	case fidlgen.HandleSubtypeNone:
		return "NullableHandle"
	case fidlgen.HandleSubtypeChannel:
		return "Channel"
	case fidlgen.HandleSubtypeEvent:
		return "Event"
	default:
		panic(fmt.Sprintf("unsupported handle subtype: %s", subtype))
	}
}

func wrapNullable(decl mixer.Declaration, valueStr string) string {
	if !decl.IsNullable() {
		return valueStr
	}
	switch decl.(type) {
	case *mixer.ArrayDecl, *mixer.VectorDecl, *mixer.StringDecl, *mixer.HandleDecl, *mixer.ClientEndDecl, *mixer.ServerEndDecl:
		return fmt.Sprintf("Some(%s)", valueStr)
	case *mixer.StructDecl, *mixer.UnionDecl:
		return fmt.Sprintf("Some(Box::new(%s))", valueStr)
	case *mixer.BoolDecl, *mixer.IntegerDecl, *mixer.FloatDecl, *mixer.TableDecl:
		panic(fmt.Sprintf("decl %v should not be nullable", decl))
	}
	panic(fmt.Sprintf("unexpected decl %v", decl))
}

func onStruct(value ir.Record, decl *mixer.StructDecl) string {
	var structFields []string
	providedKeys := make(map[string]struct{}, len(value.Fields))
	for _, field := range value.Fields {
		if field.Key.IsUnknown() {
			panic("unknown field not supported")
		}
		providedKeys[field.Key.Name] = struct{}{}
		fieldName := fidlgen.ToSnakeCase(field.Key.Name)
		fieldValueStr := visit(field.Value, decl.Field(field.Key.Name))
		structFields = append(structFields, fmt.Sprintf("%s: %s", fieldName, fieldValueStr))
	}
	for _, key := range decl.FieldNames() {
		if _, ok := providedKeys[key]; !ok {
			fieldName := fidlgen.ToSnakeCase(key)
			structFields = append(structFields, fmt.Sprintf("%s: None", fieldName))
		}
	}
	valueStr := fmt.Sprintf("%s { %s }", declName(decl), strings.Join(structFields, ", "))
	return wrapNullable(decl, valueStr)
}

func onTable(value ir.Record, decl *mixer.TableDecl) string {
	var tableFields []string
	for _, field := range value.Fields {
		if field.Key.IsUnknown() {
			panic(fmt.Sprintf("table %s: unknown ordinal %d: Rust cannot construct tables with unknown fields",
				decl.Name(), field.Key.UnknownOrdinal))
		}
		fieldName := fidlgen.ToSnakeCase(field.Key.Name)
		fieldValueStr := visit(field.Value, decl.Field(field.Key.Name))
		tableFields = append(tableFields, fmt.Sprintf("%s: Some(%s)", fieldName, fieldValueStr))
	}
	tableName := declName(decl)
	tableFields = append(tableFields, "..Default::default()")
	valueStr := fmt.Sprintf("%s { %s }", tableName, strings.Join(tableFields, ", "))
	return wrapNullable(decl, valueStr)
}

func onUnion(value ir.Record, decl *mixer.UnionDecl) string {
	if len(value.Fields) != 1 {
		panic(fmt.Sprintf("union has %d fields, expected 1", len(value.Fields)))
	}
	field := value.Fields[0]
	var valueStr string
	if field.Key.IsUnknown() {
		if field.Key.UnknownOrdinal != 0 {
			panic(fmt.Sprintf("union %s: unknown ordinal %d: Rust can only construct unknowns with the ordinal 0",
				decl.Name(), field.Key.UnknownOrdinal))
		}
		if field.Value != nil {
			panic(fmt.Sprintf("union %s: unknown ordinal %d: Rust cannot construct union with unknown bytes/handles",
				decl.Name(), field.Key.UnknownOrdinal))
		}
		valueStr = fmt.Sprintf("%s::unknown_variant_for_testing()", declName(decl))
	} else {
		fieldName := fidlgen.ToUpperCamelCase(field.Key.Name)
		fieldValueStr := visit(field.Value, decl.Field(field.Key.Name))
		valueStr = fmt.Sprintf("%s::%s(%s)", declName(decl), fieldName, fieldValueStr)
	}
	return wrapNullable(decl, valueStr)
}

func onList(value []ir.Value, decl mixer.ListDeclaration) string {
	var elements []string
	elemDecl := decl.Elem()
	for _, item := range value {
		elements = append(elements, visit(item, elemDecl))
	}
	elementsStr := strings.Join(elements, ", ")
	var valueStr string
	switch decl.(type) {
	case *mixer.ArrayDecl:
		valueStr = fmt.Sprintf("[%s]", elementsStr)
	case *mixer.VectorDecl:
		valueStr = fmt.Sprintf("vec![%s]", elementsStr)
	default:
		panic(fmt.Sprintf("unexpected decl %v", decl))
	}
	return wrapNullable(decl, valueStr)
}

func buildHandleValue(handle ir.Handle) string {
	return fmt.Sprintf("copy_handle(&handle_defs[%d])", handle)
}

func onDecodedRecord(value ir.DecodedRecord, decl mixer.RecordDeclaration) string {
	if value.Encoding.WireFormat != ir.V2WireFormat {
		panic("Rust backend only supports V2 in decode() function")
	}
	name := declName(decl)
	bytes := rust.BuildBytes(value.Encoding.Bytes)
	handles := "[]"
	if len(value.Encoding.Handles) > 0 {
		handles = fmt.Sprintf("select_handle_infos(&handle_defs, &%s)",
			buildHandles(value.Encoding.Handles))
	}
	context := encodingContext(value.Encoding.WireFormat)
	return fmt.Sprintf("decode_value::<%s>(%s, &%s, &mut %s)", name, context, bytes, handles)
}
