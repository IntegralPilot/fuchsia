// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package llcpp

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/cpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type HandleRepr int

const (
	_ = iota
	HandleReprDisposition
	HandleReprInfo
	HandleReprRaw
)

func typeNameImpl(decl mixer.Declaration, ignoreNullable bool) string {
	switch decl := decl.(type) {
	case mixer.PrimitiveDeclaration:
		return primitiveTypeName(decl.Subtype())
	case *mixer.StringDecl:
		return "fidl::StringView"
	case *mixer.StructDecl:
		if !ignoreNullable && decl.IsNullable() {
			return fmt.Sprintf("fidl::ObjectView<%s>", declName(decl))
		}
		return declName(decl)
	case *mixer.ArrayDecl:
		return fmt.Sprintf("fidl::Array<%s, %d>", typeName(decl.Elem()), decl.Size())
	case *mixer.VectorDecl:
		return fmt.Sprintf("fidl::VectorView<%s>", typeName(decl.Elem()))
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
		return fmt.Sprintf("fidl::ClientEnd<%s>", cpp.EndpointDeclName(decl))
	case *mixer.ServerEndDecl:
		return fmt.Sprintf("fidl::ServerEnd<%s>", cpp.EndpointDeclName(decl))
	case *mixer.UnionDecl:
		if !ignoreNullable && decl.IsNullable() {
			return fmt.Sprintf("fidl::WireOptional<%s>", declName(decl))
		}
		return declName(decl)
	case mixer.NamedDeclaration:
		return declName(decl)
	default:
		panic("unhandled case")
	}
}

func typeName(decl mixer.Declaration) string {
	return typeNameImpl(decl, false)
}

func typeNameIgnoreNullable(decl mixer.Declaration) string {
	return typeNameImpl(decl, true)
}

func declName(decl mixer.NamedDeclaration) string {
	// Note: only works for domain objects (not protocols & services)
	parts := strings.SplitN(decl.Name(), "/", 2)
	return fmt.Sprintf("%s::wire::%s", strings.ReplaceAll(parts[0], ".", "_"), fidlgen.ToUpperCamelCase(parts[1]))
}

func ConformanceType(gidlTypeString string) string {
	// Note: only works for domain objects (not protocols & services)
	return "test_conformance::wire::" + fidlgen.ToUpperCamelCase(gidlTypeString)
}

func LlcppErrorCode(code ir.ErrorCode) string {
	if code == ir.TooFewBytesInPrimaryObject || code == ir.TooFewBytes || code == ir.EnvelopeBytesExceedMessageLength {
		return "ZX_ERR_BUFFER_TOO_SMALL"
	}
	// TODO(https://fxbug.dev/42110793) Implement different codes for different FIDL error cases.
	return "ZX_ERR_INVALID_ARGS"
}
