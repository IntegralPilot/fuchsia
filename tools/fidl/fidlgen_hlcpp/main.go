// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/fidlgen_hlcpp/codegen"
	"go.fuchsia.dev/fuchsia/tools/fidl/fidlgen_hlcpp/coding_tables"
	cpp "go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen_cpp"
)

func main() {
	flags := cpp.NewCmdlineFlags("hlcpp", []string{})
	fidl := flags.ParseAndLoadIR()
	root := cpp.Compile(fidl)
	tables := coding_tables.Compile(fidl)
	generator := codegen.NewGenerator(flags, template.FuncMap{
		"GetCodingTables": func() coding_tables.Root { return tables },
	})
	generator.GenerateFiles(root, []string{"Header", "Implementation", "TestBase", "CodingTables"})
}
