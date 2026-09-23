// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"fmt"
	"strings"

	"go.starlark.net/syntax"
)

// tagsToGN converts Bazel tags attribute to GN assignments if known syncable tags are found.
func tagsToGN(expr syntax.Expr) ([]string, error) {
	// Currently only list literals are supported, i.e. select is not supported yet.
	listExpr, ok := expr.(*syntax.ListExpr)
	if !ok {
		return nil, fmt.Errorf("expected list literal for tags attribute, got %T", expr)
	}

	var assertNoDeps []syntax.Expr
	for _, elm := range listExpr.List {
		lit, ok := elm.(*syntax.Literal)
		if !ok {
			return nil, fmt.Errorf("expected list item to be a literal in tags attribute, got %T", elm)
		}
		// lit.Raw is quoted, so unquote.
		val := lit.Raw[1 : len(lit.Raw)-1]
		if strings.HasPrefix(val, "assert_no_deps=") {
			label := strings.TrimPrefix(val, "assert_no_deps=")
			// Requote the label for bazelDepToGN.
			depLit := &syntax.Literal{Raw: fmt.Sprintf(`"%s"`, label)}
			transformedDep, err := bazelDepToGN(depLit)
			if err != nil {
				return nil, fmt.Errorf("transforming assert_no_deps label %q: %v", label, err)
			}
			transformedDep, err = bazelVisibilityToGN(transformedDep)
			if err != nil {
				return nil, fmt.Errorf("transforming assert_no_deps wildcard %q: %v", label, err)
			}
			assertNoDeps = append(assertNoDeps, transformedDep)
		}
	}

	if len(assertNoDeps) == 0 {
		return nil, nil
	}

	newListExpr := &syntax.ListExpr{List: assertNoDeps}
	lines, err := listExprToGN(newListExpr, nil)
	if err != nil {
		return nil, fmt.Errorf("converting assert_no_deps list to GN: %v", err)
	}

	if len(lines) > 0 {
		lines[0] = "assert_no_deps = " + lines[0]
	}
	return lines, nil
}
