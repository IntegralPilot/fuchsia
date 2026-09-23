// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package legacy_syscall_cdecl

import (
	"embed"
	"path/filepath"
	"sort"
	"strings"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither"
	"go.fuchsia.dev/fuchsia/zircon/tools/zither/backends/kernel"
)

//go:embed templates/*
var templates embed.FS

type Generator struct {
	fidlgen.Generator
}

func NewGenerator(formatter fidlgen.Formatter) *Generator {
	gen := fidlgen.NewGenerator("LegacySyscallCDeclTemplates", templates, formatter, template.FuncMap{
		"LegacySyscallCDecl": func(syscall zither.Syscall) string {
			return kernel.SyscallCDecl(syscall, kernel.PointerViewUserspace, func(zither.Syscall) string { return "_ZX_SYSCALL_DECL" })
		},
	})
	return &Generator{*gen}
}

func (gen Generator) DeclOrder() zither.DeclOrder { return zither.SourceDeclOrder }

func (gen Generator) DeclCallback(zither.Decl) {}

func (gen *Generator) Generate(summary zither.LibrarySummary, outputDir string) ([]string, error) {
	var public, testonly, next []zither.Syscall
	for _, summary := range summary.Files {
		for _, decl := range summary.Decls {
			if !decl.IsSyscallFamily() {
				continue
			}
			for _, syscall := range decl.AsSyscallFamily().Syscalls {
				if syscall.IsInternal() {
					continue
				}
				if syscall.Testonly {
					testonly = append(testonly, syscall)
				} else if syscall.IsNext() {
					next = append(next, syscall)
				} else {
					public = append(public, syscall)
				}
			}
		}
	}

	var outputs []string
	for _, file := range []struct {
		name        string
		Counterpart string
		Syscalls    []zither.Syscall
	}{
		{"cdecls.inc", "<zircon/syscalls.h>", public},
		{"testonly-cdecls.inc", "<zircon/testonly-syscalls.h>", testonly},
		{"cdecls-next.inc", "<zircon/syscalls-next.h>", next},
	} {
		output := filepath.Join(outputDir, "zircon", "syscalls", "gen", file.name)
		sort.Slice(file.Syscalls, func(i, j int) bool {
			return strings.Compare(file.Syscalls[i].Name, file.Syscalls[j].Name) < 0
		})
		if err := gen.GenerateFile(output, "GenerateCDecls", file); err != nil {
			return nil, err
		}
		outputs = append(outputs, output)
	}
	return outputs, nil
}
