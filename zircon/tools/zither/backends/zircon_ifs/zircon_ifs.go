// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package zircon_ifs

import (
	"embed"
	"encoding/json"
	"path/filepath"
	"sort"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither"
)

//go:embed templates/*
var templates embed.FS

type Generator struct {
	fidlgen.Generator
}

func NewGenerator(formatter fidlgen.Formatter) *Generator {
	gen := fidlgen.NewGenerator("ZirconIfsTemplates", templates, formatter, template.FuncMap{})
	return &Generator{*gen}
}

func (gen Generator) DeclOrder() zither.DeclOrder { return zither.SourceDeclOrder }

func (gen Generator) DeclCallback(zither.Decl) {}

func (gen *Generator) Generate(summary zither.LibrarySummary, outputDir string) ([]string, error) {
	syscall_names := []string{
		// TODO(https://fxbug.dev/42126998): These are not syscalls,
		// but are a part of the vDSO interface today (they probably
		// shouldn't be). For now, we hardcode these symbols here.
		"zx_exception_get_string",
		"zx_status_get_string",
	}
	for _, summary := range summary.Files {
		for _, decl := range summary.Decls {
			if !decl.IsSyscallFamily() {
				continue
			}
			family := decl.AsSyscallFamily()
			for _, syscall := range family.Syscalls {
				if syscall.IsInternal() {
					continue
				}
				name := "zx_" + zither.LowerCaseWithUnderscores(syscall)
				syscall_names = append(syscall_names, name)
			}
		}
	}
	sort.Strings(syscall_names)

	symbols := []symbol{}
	for _, name := range syscall_names {
		symbols = append(symbols,
			symbol{Name: "_" + name, Weak: false},
			symbol{Name: name, Weak: true},
		)
	}
	sort.Slice(symbols, func(i, j int) bool {
		return strings.Compare(symbols[i].Name, symbols[j].Name) < 0
	})

	ifs_output := filepath.Join(outputDir, "zircon.ifs")
	if err := gen.GenerateFile(ifs_output, "GenerateZirconIfsFile", symbols); err != nil {
		return nil, err
	}

	// json formatting:
	// https://fuchsia.dev/reference/tools/fx/cmd/format-code
	json_output := filepath.Join(outputDir, "libzircon.json")
	json_bytes, err := json.MarshalIndent(syscall_names, "", "    ")
	if err != nil {
		return nil, err
	}
	if err := fidlgen.WriteFileIfChanged(json_output, append(json_bytes, '\n')); err != nil {
		return nil, err
	}

	return []string{ifs_output, json_output}, nil
}

type symbol struct {
	Name string
	Weak bool
}
