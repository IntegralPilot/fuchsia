// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"fmt"
	"strings"

	"go.starlark.net/syntax"
)

// rustenvToGN converts Bazel rustc_env dictionary attribute to GN rustenv list of strings.
func rustenvToGN(expr syntax.Expr) ([]string, error) {
	dictExpr, ok := expr.(*syntax.DictExpr)
	if !ok {
		return nil, fmt.Errorf("expected dictionary literal for rustc_env attribute, got %T", expr)
	}

	var envVarExprs []syntax.Expr
	for _, entry := range dictExpr.List {
		dictEntry, ok := entry.(*syntax.DictEntry)
		if !ok {
			return nil, fmt.Errorf("unexpected node type in dictionary entry: %T", entry)
		}

		keyLit, ok := dictEntry.Key.(*syntax.Literal)
		if !ok {
			return nil, fmt.Errorf("expected dictionary key to be a literal, got %T", dictEntry.Key)
		}
		key := strings.Trim(keyLit.Raw, `"`)

		valLit, ok := dictEntry.Value.(*syntax.Literal)
		if !ok {
			return nil, fmt.Errorf("expected dictionary value to be a literal, got %T", dictEntry.Value)
		}
		val := strings.Trim(valLit.Raw, `"`)
		if ow, ok := overwrittenValue(dictEntry); ok {
			val = ow
		}

		envVarExprs = append(envVarExprs, &syntax.Literal{Raw: fmt.Sprintf(`"%s=%s"`, key, val)})
	}

	if len(envVarExprs) == 0 {
		return nil, nil
	}

	newListExpr := &syntax.ListExpr{List: envVarExprs}
	lines, err := listExprToGN(newListExpr, nil)
	if err != nil {
		return nil, fmt.Errorf("converting rustenv list to GN: %v", err)
	}

	if len(lines) > 0 {
		lines[0] = "rustenv = " + lines[0]
	}
	return lines, nil
}
