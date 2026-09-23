// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/cpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/driver_cpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/driver_llcpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/dynfidl"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/fuzzer_corpus"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/golang"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/hlcpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/llcpp"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/python"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/reference"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/rust"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/rust_codec"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/backend/rust_next"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/cli"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
)

var generators = map[ir.OutputType]map[ir.Language]cli.Generator{
	ir.OutputTypeConformance: {
		ir.LanguageCpp:          cpp.GenerateConformanceTests,
		ir.LanguageDynfidl:      dynfidl.GenerateConformanceTests,
		ir.LanguageFuzzerCorpus: fuzzer_corpus.GenerateConformanceTests,
		ir.LanguageGo:           golang.GenerateConformanceTests,
		ir.LanguageHlcpp:        hlcpp.GenerateConformanceTests,
		ir.LanguageLlcpp:        llcpp.GenerateConformanceTests,
		ir.LanguagePython:       python.GenerateConformanceTests,
		ir.LanguageRust:         rust.GenerateConformanceTests,
		ir.LanguageRustCodec:    rust_codec.GenerateConformanceTests,
		ir.LanguageRustNext:     rust_next.GenerateConformanceTests,
	},
	ir.OutputTypeBenchmark: {
		ir.LanguageCpp:         cpp.GenerateBenchmarks,
		ir.LanguageDriverCpp:   driver_cpp.GenerateBenchmarks,
		ir.LanguageDriverLlcpp: driver_llcpp.GenerateBenchmarks,
		ir.LanguageGo:          golang.GenerateBenchmarks,
		ir.LanguageHlcpp:       hlcpp.GenerateBenchmarks,
		ir.LanguageLlcpp:       llcpp.GenerateBenchmarks,
		ir.LanguageReference:   reference.GenerateBenchmarks,
		ir.LanguageRust:        rust.GenerateBenchmarks,
	},
	ir.OutputTypeMeasureTape: {
		ir.LanguageRust: rust.GenerateMeasureTapeTests,
	},
}

func main() {
	cli.Main(generators)
}
