// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package codegen

import (
	"embed"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

//go:embed *.tmpl
var templates embed.FS

type Generator struct {
	fdomain bool
	common  bool
	*fidlgen.Generator
}

func NewGenerator(rustfmtPath, rustfmtConfigPath string, fdomain bool, common bool, use_common string, apiCoverage bool) *Generator {
	var args []string
	if rustfmtConfigPath != "" {
		args = append(args, "--config-path", rustfmtConfigPath)
	}
	formatter := fidlgen.NewFormatter(rustfmtPath, args...)

	return &Generator{
		fdomain: fdomain,
		common:  common,
		Generator: fidlgen.NewGenerator("RustTemplates", templates, formatter, template.FuncMap{
			"ApiCoverage": func() bool { return apiCoverage },
			"ResourceDialect": func() string {
				if fdomain {
					return "fdomain_client::fidl::FDomainResourceDialect"
				} else {
					return "fidl::encoding::DefaultFuchsiaResourceDialect"
				}
			},
			"FDomain":     func() bool { return fdomain },
			"IsCommon":    func() bool { return common },
			"UseCommon":   func() string { return use_common },
			"UsingCommon": func() bool { return use_common != "" },
			"EmitType": func(is_value bool) bool {
				if is_value {
					return common || use_common == ""
				} else {
					return !common
				}
			},
			"MarkerNamespace": func() string {
				if fdomain {
					return "fdomain_client::fidl"
				} else {
					return "fidl::endpoints"
				}
			},
			"ChannelType": func() string {
				if fdomain {
					return "fdomain_client::Channel"
				} else {
					return "::fidl::AsyncChannel"
				}
			},
		})}
}

func (gen *Generator) GenerateFidl(ir fidlgen.Root, outputFilename string, includeDrivers bool) error {
	tree := Compile(ir, includeDrivers, gen.fdomain, gen.common)
	return gen.GenerateFile(outputFilename, "GenerateSourceFile", tree)
}
