// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust_codec

import (
	"bytes"
	_ "embed"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/config"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/rust"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

var (
	//go:embed conformance.tmpl
	conformanceTmplText string

	conformanceTmpl = template.Must(template.New("conformanceTmpl").Parse(conformanceTmplText))
)

type conformanceTmplInput struct {
	EncodeSuccessCases []encodeSuccessCase
	DecodeSuccessCases []decodeSuccessCase
	EncodeFailureCases []encodeFailureCase
	DecodeFailureCases []decodeFailureCase
}

type encodeSuccessCase struct {
	Name, HandleDefs, DeclName, Value, Bytes, Handles, HandleDispositions string
}

type decodeSuccessCase struct {
	Name, HandleDefs, DeclName, Bytes, Handles, UnusedHandles, ConfirmValue string
}

type encodeFailureCase struct {
	Name, HandleDefs, DeclName, Value string
}

type decodeFailureCase struct {
	Name, HandleDefs, DeclName, Bytes, Handles string
}

// GenerateConformanceTests generates Rust tests.
func GenerateConformanceTests(gidl ir.All, fidl fidlgen.Root, config config.GeneratorConfig) ([]byte, error) {
	schema := mixer.BuildSchema(fidl)
	encodeSuccessCases, err := encodeSuccessCases(gidl.EncodeSuccess, schema)
	if err != nil {
		return nil, err
	}
	decodeSuccessCases, err := decodeSuccessCases(gidl.DecodeSuccess, schema)
	if err != nil {
		return nil, err
	}
	encodeFailureCases, err := encodeFailureCases(gidl.EncodeFailure, schema)
	if err != nil {
		return nil, err
	}
	decodeFailureCases, err := decodeFailureCases(gidl.DecodeFailure, schema)
	if err != nil {
		return nil, err
	}
	input := conformanceTmplInput{
		EncodeSuccessCases: encodeSuccessCases,
		DecodeSuccessCases: decodeSuccessCases,
		EncodeFailureCases: encodeFailureCases,
		DecodeFailureCases: decodeFailureCases,
	}
	var buf bytes.Buffer
	err = conformanceTmpl.Execute(&buf, input)
	return buf.Bytes(), err
}

func encodeSuccessCases(gidlEncodeSuccesses []ir.EncodeSuccess, schema mixer.Schema) ([]encodeSuccessCase, error) {
	var encodeSuccessCases []encodeSuccessCase
	for _, encodeSuccess := range gidlEncodeSuccesses {
		decl, err := schema.ExtractDeclarationEncodeSuccess(encodeSuccess.Value, encodeSuccess.HandleDefs)
		if err != nil {
			return nil, fmt.Errorf("encode success %s: %s", encodeSuccess.Name, err)
		}
		declName := decl.Name()
		value := visit(encodeSuccess.Value, decl)
		for _, encoding := range encodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			newCase := encodeSuccessCase{
				Name:       testCaseName(encodeSuccess.Name, encoding.WireFormat),
				HandleDefs: buildHandleDefs(encodeSuccess.HandleDefs),
				DeclName:   declName,
				Value:      value,
				Bytes:      rust.BuildBytes(encoding.Bytes),
			}
			if len(newCase.HandleDefs) != 0 {
				if encodeSuccess.CheckHandleRights {
					newCase.HandleDispositions = buildRawHandleDispositions(encoding.HandleDispositions)
				} else {
					newCase.Handles = buildRawHandles(encoding.HandleDispositions)
				}
			}
			encodeSuccessCases = append(encodeSuccessCases, newCase)
		}
	}
	return encodeSuccessCases, nil
}

func decodeSuccessCases(gidlDecodeSuccesses []ir.DecodeSuccess, schema mixer.Schema) ([]decodeSuccessCase, error) {
	var decodeSuccessCases []decodeSuccessCase
	for _, decodeSuccess := range gidlDecodeSuccesses {
		decl, err := schema.ExtractDeclaration(decodeSuccess.Value, decodeSuccess.HandleDefs)
		if err != nil {
			return nil, fmt.Errorf("decode success %s: %s", decodeSuccess.Name, err)
		}
		declName := decl.Name()
		confirmValue := visit(decodeSuccess.Value, decl)
		for _, encoding := range decodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			unusedHandles := ""
			if h := ir.GetUnusedHandles(decodeSuccess.Value, encoding.Handles); len(h) != 0 {
				unusedHandles = buildHandles(h)
			}
			decodeSuccessCases = append(decodeSuccessCases, decodeSuccessCase{
				Name:          testCaseName(decodeSuccess.Name, encoding.WireFormat),
				ConfirmValue:  confirmValue,
				HandleDefs:    buildHandleDefs(decodeSuccess.HandleDefs),
				DeclName:      declName,
				Bytes:         rust.BuildBytes(encoding.Bytes),
				Handles:       buildHandles(encoding.Handles),
				UnusedHandles: unusedHandles,
			})
		}
	}
	return decodeSuccessCases, nil
}

func encodeFailureCases(gidlEncodeFailures []ir.EncodeFailure, schema mixer.Schema) ([]encodeFailureCase, error) {
	var encodeFailureCases []encodeFailureCase
	for _, encodeFailure := range gidlEncodeFailures {
		decl, err := schema.ExtractDeclarationUnsafe(encodeFailure.Value)
		if err != nil {
			return nil, fmt.Errorf("encode failure %s: %s", encodeFailure.Name, err)
		}
		declName := decl.Name()
		value := visit(encodeFailure.Value, decl)

		for _, wireFormat := range supportedWireFormats {
			encodeFailureCases = append(encodeFailureCases, encodeFailureCase{
				Name:       testCaseName(encodeFailure.Name, wireFormat),
				HandleDefs: buildHandleDefs(encodeFailure.HandleDefs),
				DeclName:   declName,
				Value:      value,
			})
		}
	}
	return encodeFailureCases, nil
}

func decodeFailureCases(gidlDecodeFailures []ir.DecodeFailure, schema mixer.Schema) ([]decodeFailureCase, error) {
	var decodeFailureCases []decodeFailureCase
	for _, decodeFailure := range gidlDecodeFailures {
		decl, err := schema.ExtractDeclarationByName(decodeFailure.Type)
		if err != nil {
			return nil, fmt.Errorf("decode failure %s: %s", decodeFailure.Name, err)
		}
		declName := decl.Name()
		for _, encoding := range decodeFailure.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeFailureCases = append(decodeFailureCases, decodeFailureCase{
				Name:       testCaseName(decodeFailure.Name, encoding.WireFormat),
				HandleDefs: buildHandleDefs(decodeFailure.HandleDefs),
				DeclName:   declName,
				Bytes:      rust.BuildBytes(encoding.Bytes),
				Handles:    buildHandles(encoding.Handles),
			})
		}
	}
	return decodeFailureCases, nil
}

func visit(value ir.Value, decl mixer.Declaration) string {
	switch value := value.(type) {
	case bool:
		return fmt.Sprintf("Value::Bool(%s)", strconv.FormatBool(value))
	case int64, uint64, float64:
		switch decl := decl.(type) {
		case mixer.PrimitiveDeclaration:
			wrapper := primitiveTypeValueWrapper(decl.Subtype())
			return fmt.Sprintf("Value::%s(%v%s)", wrapper, value, primitiveTypeName(decl.Subtype()))
		case *mixer.BitsDecl:
			return fmt.Sprintf("Value::Bits(\"%s\".to_owned(), Box::new(%s))", decl.Name(), visit(value, &decl.Underlying))
		case *mixer.EnumDecl:
			return fmt.Sprintf("Value::Enum(\"%s\".to_owned(), Box::new(%s))", decl.Name(), visit(value, &decl.Underlying))
		}
	case ir.RawFloat:
		switch decl.(*mixer.FloatDecl).Subtype() {
		case fidlgen.Float32:
			return fmt.Sprintf("Value::F32(f32::from_bits(%#b))", value)
		case fidlgen.Float64:
			return fmt.Sprintf("Value::F64(f64::from_bits(%#b))", value)
		}
	case string:
		if fidlgen.PrintableASCII(value) {
			return fmt.Sprintf("Value::String(String::from(%q))", value)
		} else {
			return fmt.Sprintf("std::str::from_utf8(b\"%s\").map(|s| Value::String(s.to_owned())).unwrap_or(Value::Null)", rust.EscapeStr(value))
		}
	case ir.Handle:
		expr := buildHandleValue(value)
		rights := buildHandleRights(value)
		switch decl := decl.(type) {
		case *mixer.ClientEndDecl:
			return fmt.Sprintf("Value::ClientEnd(%s, \"%s\".to_owned(), %s)", expr, decl.ProtocolName(), rights)
		case *mixer.ServerEndDecl:
			return fmt.Sprintf("Value::ServerEnd(%s, \"%s\".to_owned(), %s)", expr, decl.ProtocolName(), rights)
		case *mixer.HandleDecl:
			ty := "None"

			if decl.Subtype() != "handle" {
				ty = fidlgen.ToUpperCamelCase(string(decl.Subtype()))
			}
			return fmt.Sprintf("Value::Handle(%s, fidl_codec::ObjectType::%s, %s)", expr, ty, rights)
		}
	case ir.RestrictedHandle:
		expr := buildHandleValue(value.Handle)
		rights := fmt.Sprintf("Some(fidl_codec::Rights::from_bits(%d).unwrap())", value.Rights)
		switch decl := decl.(type) {
		case *mixer.ClientEndDecl:
			return fmt.Sprintf("Value::ClientEnd(%s, \"%s\".to_owned(), %s)", expr, decl.ProtocolName(), rights)
		case *mixer.ServerEndDecl:
			return fmt.Sprintf("Value::ServerEnd(%s, \"%s\".to_owned(), %s)", expr, decl.ProtocolName(), rights)
		case *mixer.HandleDecl:
			ty := "None"

			if decl.Subtype() != "handle" {
				ty = fidlgen.ToUpperCamelCase(string(decl.Subtype()))
			}
			return fmt.Sprintf("Value::Handle(%s, fidl_codec::ObjectType::%s, %s)", expr, ty, rights)
		}
	case ir.Record:
		switch decl := decl.(type) {
		case *mixer.StructDecl:
			content := ""
			for _, field := range decl.FieldNames() {
				field_value := "Value::Null"
				for _, value_field := range value.Fields {
					if value_field.Key.Name != field {
						continue
					}
					field_decl := decl.Field(value_field.Key.Name)
					field_value = visit(value_field.Value, field_decl)
				}

				if content != "" {
					content += ", "
				}
				content += fmt.Sprintf("(\"%s\".to_owned(), %s)", field, field_value)
			}
			return fmt.Sprintf("Value::Object(vec![%s])", content)
		case *mixer.TableDecl:
			content := ""
			for _, field := range value.Fields {
				field_decl := decl.Field(field.Key.Name)
				if content != "" {
					content += ", "
				}
				content += fmt.Sprintf("(\"%s\".to_owned(), %s)", field.Key.Name, visit(field.Value, field_decl))
			}
			return fmt.Sprintf("Value::Object(vec![%s])", content)
		case *mixer.UnionDecl:
			field := value.Fields[0]
			field_decl := decl.Field(field.Key.Name)
			return fmt.Sprintf("Value::Union(\"%s\".to_owned(), \"%s\".to_owned(), Box::new(%s))", decl.Name(), field.Key.Name, visit(field.Value, field_decl))
		}
	case []ir.Value:
		var value_decl mixer.Declaration
		switch decl := decl.(type) {
		case *mixer.ArrayDecl:
			value_decl = decl.Elem()
		case *mixer.VectorDecl:
			value_decl = decl.Elem()
		}

		content := ""
		for _, item := range value {
			if content != "" {
				content += ", "
			}
			content += visit(item, value_decl)
		}
		return fmt.Sprintf("Value::List(vec![%s])", content)
	case ir.DecodedRecord:
		return "Value::Null"
	case nil:
		if !decl.IsNullable() {
			if _, ok := decl.(*mixer.HandleDecl); ok {
				return "Value::Handle(fidl_codec::NullableHandle::invalid(), fidl_codec::ObjectType::None, None)"
			}
			panic(fmt.Sprintf("got nil for non-nullable type: %T", decl))
		}
		return "Value::Null"
	}
	panic(fmt.Sprintf("not implemented: %T", value))
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

func primitiveTypeValueWrapper(subtype fidlgen.PrimitiveSubtype) string {
	switch subtype {
	case fidlgen.Bool:
		return "Bool"
	case fidlgen.Int8:
		return "I8"
	case fidlgen.Uint8:
		return "U8"
	case fidlgen.Int16:
		return "I16"
	case fidlgen.Uint16:
		return "U16"
	case fidlgen.Int32:
		return "I32"
	case fidlgen.Uint32:
		return "U32"
	case fidlgen.Int64:
		return "I64"
	case fidlgen.Uint64:
		return "U64"
	case fidlgen.Float32:
		return "F32"
	case fidlgen.Float64:
		return "F64"
	default:
		panic(fmt.Sprintf("unexpected subtype %v", subtype))
	}
}

func testCaseName(baseName string, wireFormat ir.WireFormat) string {
	return fidlgen.ToSnakeCase(fmt.Sprintf("%s_%s", baseName, wireFormat))
}

var supportedWireFormats = []ir.WireFormat{
	ir.V2WireFormat,
}

func wireFormatSupported(wireFormat ir.WireFormat) bool {
	return slices.Contains(supportedWireFormats, wireFormat)
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

func buildHandleValue(handle ir.Handle) string {
	return fmt.Sprintf("copy_handle(&handle_defs[%d])", handle)
}

func buildHandleRights(handle ir.Handle) string {
	return fmt.Sprintf("Some(fidl_codec::Rights::from_bits(handle_defs[%d].rights).unwrap())", handle)
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
