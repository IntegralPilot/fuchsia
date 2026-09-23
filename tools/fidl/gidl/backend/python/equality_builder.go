// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
package python

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

func buildEqualityCheck(expectedValue ir.Value, decl mixer.Declaration) string {
	var b equalityCheckBuilder
	b.visit("value", expectedValue, decl)
	return b.String()
}

func canAssertEq(value ir.Value) bool {
	switch value := value.(type) {
	case nil, string, bool, int64, uint64, float64:
		return true
	case ir.RawFloat, ir.AnyHandle, ir.UnknownData:
		return false
	case []ir.Value:
		for _, elem := range value {
			if !canAssertEq(elem) {
				return false
			}
		}
		return true
	case ir.Record:
		for _, field := range value.Fields {
			if field.Key.IsUnknown() || !canAssertEq(field.Value) {
				return false
			}
		}
		return true
	}
	panic(fmt.Sprintf("unhandled type: %T", value))
}

type equalityCheckBuilder struct {
	strings.Builder
}

func (b *equalityCheckBuilder) write(format string, args ...any) {
	// Eight spaces to fit with the Python formatting for each line.
	b.WriteString("        ")
	fmt.Fprintf(b, format, args...)
	b.WriteRune('\n')
}

func (b *equalityCheckBuilder) visit(expr string, value ir.Value, decl mixer.Declaration) {
	if decl.IsNullable() && value != nil {
		b.write("self.assertIsNotNone(%s)", expr)
	}
	if canAssertEq(value) {
		b.write("self.assertEqual(%s, %s)", expr, visit(value, decl))
		return
	}
	switch value := value.(type) {
	case ir.RawFloat:
		switch decl.(*mixer.FloatDecl).Subtype() {
		case fidlgen.Float32:
			b.write("self.assertEqual(%s, struct.unpack('>f', bytes.fromhex('%08x'))[0])", expr, value)
		case fidlgen.Float64:
			b.write("self.assertEqual(%s, struct.unpack('>d', bytes.fromhex('%016x'))[0])", expr, value)
		}
	case ir.Handle:
		b.write(fmt.Sprintf("self.assertEqual(fuchsia_controller_py.Handle(%s).koid(), handle_koids[%d])", expr, value))
	case ir.RestrictedHandle:
		// We can't check the type and rights because this information isn't preserved on emulated
		// handles on the host.
		b.write(fmt.Sprintf("self.assertEqual(fuchsia_controller_py.Handle(%s).koid(), handle_koids[%d])", expr, value.Handle))
	case []ir.Value:
		elemDecl := decl.(mixer.ListDeclaration).Elem()
		for i, elem := range value {
			b.visit(fmt.Sprintf("%s[%d]", expr, i), elem, elemDecl)
		}
	case ir.Record:
		switch decl := decl.(type) {
		case *mixer.StructDecl:
			for _, field := range value.Fields {
				fieldName := changeIfReserved(fidlgen.ToSnakeCase(field.Key.Name))
				b.visit(fmt.Sprintf("%s.%s", expr, fieldName), field.Value, decl.Field(field.Key.Name))
			}
		case *mixer.TableDecl:
			for _, field := range value.Fields {
				fieldName := changeIfReserved(fidlgen.ToSnakeCase(field.Key.Name))
				fieldExpr := fmt.Sprintf("%s.%s", expr, fieldName)
				fieldDecl := decl.Field(field.Key.Name)
				if field.Value != nil && !fieldDecl.IsNullable() {
					b.write("self.assertIsNotNone(%s)", fieldExpr)
				}
				b.visit(fieldExpr, field.Value, fieldDecl)
			}
		case *mixer.UnionDecl:
			field := value.Fields[0]
			if field.Key.IsUnknown() {
				if field.Value != nil {
					panic(fmt.Sprintf("union %s: unknown ordinal %d: Python cannot construct union with unknown bytes/handles", decl.Name(), field.Key.UnknownOrdinal))
				}
				// Unknown unions are not currently handled properly in decode, so right now this
				// codepath will not be hit.
				// TODO(b/42166276): This will be hit when handling dropped union values, so should
				// be addressed here.
				// TODO(b/332769651): Unknown values are never decoded properly so fidl_codec will
				// need to be extended to handle this area of the codebase. This will not just apply
				// to unions but also to tables, structs, and enums.
			} else {
				fieldName := changeIfReserved(fidlgen.ToSnakeCase(field.Key.Name))
				b.write("self.assertEqual(%s.%s, %s)", expr, fieldName, visit(field.Value, decl.Field(field.Key.Name)))
			}
		default:
			panic(fmt.Sprintf("unhandled decl type: %T", decl))
		}
	default:
		panic(fmt.Sprintf("unhandled value type: %T", value))
	}
}
