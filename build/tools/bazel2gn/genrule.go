// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"fmt"
	"strings"

	"go.starlark.net/syntax"
)

// genruleCmdToGN converts Bazel genrule cmd values to GN script and args.
//
// This function currently only handles the case where the cmd is a string
// literal starting with `$(location //path/to:tool)`, e.g.
//
//	cmd = "$(location //path/to:tool) arg1 arg2"
func genruleCmdToGN(expr syntax.Expr) ([]string, error) {
	lit, ok := expr.(*syntax.Literal)
	if !ok {
		return nil, fmt.Errorf("expected string literal for genrule cmd, got %T", expr)
	}

	cmd := lit.Raw
	if len(cmd) >= 2 && cmd[0] == '"' && cmd[len(cmd)-1] == '"' {
		cmd = cmd[1 : len(cmd)-1]
	}

	parts := strings.Fields(cmd)

	if len(parts) == 0 || parts[0] != "$(location" || !strings.HasSuffix(parts[1], ")") {
		return nil, fmt.Errorf("expected cmd to start with `$(location //path/to:tool)`, got %q", cmd)
	}

	script := parts[1][:len(parts[1])-1]
	args := parts[2:]

	// Files are also targets in Bazel, convert that to proper GN paths.
	// i.e. :file -> file, //path/to:file -> //path/to/file
	script = strings.TrimPrefix(script, ":")
	if idx := strings.LastIndex(script, ":"); idx != -1 {
		script = script[:idx] + "/" + script[idx+1:]
	}

	ret := []string{
		fmt.Sprintf(`script = "%s"`, script),
		"args = [",
	}

	// See available substitutions in https://bazel.build/reference/be/general#genrule.
	for _, arg := range args {
		switch arg {
		case "$@":
			ret = append(ret, "] + rebase_path(outputs, root_build_dir) + [")
		case "$<":
			ret = append(ret, "] + rebase_path(sources, root_build_dir) + [")
		default:
			ret = append(ret, indent([]string{fmt.Sprintf(`"%s",`, arg)}, 1)...)
		}
	}
	ret = append(ret, "]")
	return ret, nil
}
