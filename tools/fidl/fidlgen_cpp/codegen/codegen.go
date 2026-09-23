// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package codegen

import (
	"bytes"
	"embed"
	"fmt"
	"text/template"

	cpp "go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen_cpp"
)

type Generator struct {
	*cpp.Generator
}

type TypedArgument struct {
	ArgumentName  string
	ArgumentValue string
	ArgumentType  cpp.Type
	Pointer       bool
	Nullable      bool
	Access        bool
	MutableAccess bool
}

type closeHandleContext struct {
	unique_name_counter int
}

func (c *closeHandleContext) genUniqueName() string {
	c.unique_name_counter += 1
	return fmt.Sprintf("e_%d", c.unique_name_counter)
}

// closeHandles generates a code snippet to recursively close all handles within
// a wire domain object identified by the expression expr.
//
// exprType is the type of the expression.
func closeHandles(expr string, exprType cpp.Type, context *closeHandleContext) string {
	if !exprType.IsResource {
		return ""
	}

	switch exprType.Kind {
	case cpp.TypeKinds.Handle, cpp.TypeKinds.Endpoint:
		return fmt.Sprintf("%s.reset();", expr)
	case cpp.TypeKinds.Array, cpp.TypeKinds.Vector:
		// Iterating over array and vector views isn't affected by optionality.
		var buf bytes.Buffer
		// Use a unique item name to avoid shadowing the name of the collection or
		// the name of the item for an ancestor for loop. Shadowing can be detected
		// by compiling with -Wshadow.
		expr_item_name := context.genUniqueName()
		buf.WriteString(fmt.Sprintf("for (auto& %s : %s) {\n", expr_item_name, expr))
		buf.WriteString(closeHandles(expr_item_name, *exprType.ElementType, context))
		buf.WriteString("\n}\n")
		return buf.String()
	case cpp.TypeKinds.Union:
		// An optional union is wrapped in a `fidl::WireOptional`.
		if exprType.Nullable {
			return fmt.Sprintf("if (%s.has_value()) { %s->_CloseHandles(); }", expr, expr)
		}
		return fmt.Sprintf("%s._CloseHandles();", expr)
	default:
		// An optional struct is wrapped in a `fidl::ObjectView`.
		if exprType.Nullable {
			return fmt.Sprintf("if (%s != nullptr) { %s->_CloseHandles(); }", expr, expr)
		}
		return fmt.Sprintf("%s._CloseHandles();", expr)
	}
}

// These are the helper functions we inject for use by the templates.
var utilityFuncs = template.FuncMap{
	"SyncCallTotalStackSizeV2": func(m cpp.Method) int {
		totalSize := 0
		if m.Request.ClientAllocationV2.IsStack {
			totalSize += m.Request.ClientAllocationV2.StackBytes
		}
		if m.Response.ClientAllocationV2.IsStack {
			totalSize += m.Response.ClientAllocationV2.StackBytes
		}
		return totalSize
	},
	"CloseHandles": func(member cpp.Member, useAccessor bool) string {
		v, t := member.NameAndType()
		if useAccessor {
			v = fmt.Sprintf("%s()", v)
		}
		return closeHandles(v, t, &closeHandleContext{})
	},
}

//go:embed *.tmpl driver/*.tmpl
var templates embed.FS

func NewGenerator(flags *cpp.CmdlineFlags) *cpp.Generator {
	return cpp.NewGenerator(flags, templates, utilityFuncs)
}
