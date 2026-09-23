// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust_next

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

func buildEqualityCheck(actualExpr string, expectedValue ir.Value, decl mixer.Declaration) string {
	var b equalityCheckBuilder
	b.visit(actualExpr, expectedValue, decl)
	return b.String()
}

// Returns true if the value can be tested with a single `assert_eq!`.
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
	fmt.Fprintf(b, format, args...)
	b.WriteRune('\n')
}

func (b *equalityCheckBuilder) visit(expr string, value ir.Value, decl mixer.Declaration) {
	if canAssertEq(value) {
		b.write("assert_eq!(%s, %s);", expr, visit(value, decl))
		return
	}
	if decl.IsNullable() {
		// Unwrap the Option<...>.
		expr = fmt.Sprintf("%s.as_ref().unwrap()", expr)
		if _, ok := value.(ir.Record); ok {
			// Unwrap again for Option<Box<...>>.
			expr = fmt.Sprintf("%s.as_ref()", expr)
		}
	}
	switch value := value.(type) {
	case ir.RawFloat:
		b.write("assert_eq!(%s.to_bits(), 0x%x)", expr, value)
	case ir.Handle:
		switch decl := decl.(type) {
		case *mixer.HandleDecl:
			b.write("assert_eq!(%s.basic_info().unwrap().koid.raw_koid(), _handle_koids[%d]);", expr, value)
		case *mixer.ClientEndDecl, *mixer.ServerEndDecl:
			b.write("assert_eq!(%s.as_ref().into_untyped().basic_info().unwrap().koid.raw_koid(), _handle_koids[%d]);", expr, value)
		default:
			panic(fmt.Sprintf("unhandled decl type: %T", decl))
		}
	case ir.AnyHandle:
		// do nothing
	case ir.RestrictedHandle:
		b.write(`
match %s.basic_info() {
	Ok(info) => {
		assert_eq!(info.koid.raw_koid(), _handle_koids[%d]);
		assert_eq!(info.object_type, %s);
		assert_eq!(info.rights, Rights::from_bits(%d).unwrap());
	},
	Err(e) => panic!("handle basic_info failed: {}", e),
}
`, expr, value.Handle, objectTypeConst(value.Type), value.Rights)
	case []ir.Value:
		elemDecl := decl.(mixer.ListDeclaration).Elem()
		for i, elem := range value {
			b.visit(fmt.Sprintf("%s[%d]", expr, i), elem, elemDecl)
		}
	case ir.Record:
		switch decl := decl.(type) {
		case *mixer.StructDecl:
			for _, field := range value.Fields {
				b.visit(fmt.Sprintf("%s.%s", expr, field.Key.Name), field.Value, decl.Field(field.Key.Name))
			}
		case *mixer.TableDecl:
			for _, field := range value.Fields {
				b.visit(fmt.Sprintf("%s.%s.unwrap()", expr, field.Key.Name), field.Value, decl.Field(field.Key.Name))
			}
		case *mixer.UnionDecl:
			field := value.Fields[0]
			if field.Key.IsUnknown() {
				if field.Value != nil {
					panic(fmt.Sprintf("union %s: unknown ordinal %d: Rust cannot construct union with unknown bytes/handles",
						decl.Name(), field.Key.UnknownOrdinal))
				}
				b.write("assert!(%s.is_unknown());", expr)
			} else {
				b.write(`
	match %s {
		%s::%s(x) => {
	`, expr, declName(decl), fidlgen.ToUpperCamelCase(field.Key.Name))
				b.visit("x", field.Value, decl.Field(field.Key.Name))
				b.write("}")
				if len(decl.FieldNames()) > 1 {
					b.write(`other => panic!("expected %s::%s, got {:?}", other)`, expr, declName(decl))
				}
				b.write("}")
			}
		default:
			panic(fmt.Sprintf("unhandled decl type: %T", decl))
		}
	default:
		panic(fmt.Sprintf("unhandled value type: %T", value))
	}
}

func objectTypeConst(objectType fidlgen.ObjectType) string {
	switch objectType {
	case fidlgen.ObjectTypeEvent:
		return "zx::ObjectType::EVENT"
	case fidlgen.ObjectTypeChannel:
		return "zx::ObjectType::CHANNEL"
	case fidlgen.ObjectTypeSocket:
		return "zx::ObjectType::SOCKET"
	default:
		panic(fmt.Sprintf("unsupported object type: %d", objectType))
	}
}
