// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"flag"
	"fmt"
	"log"
	"os"

	"google.golang.org/protobuf/encoding/prototext"

	"go.fuchsia.dev/fuchsia/tools/bazel-docgen"
	pb "go.fuchsia.dev/fuchsia/tools/bazel-docgen/third_party/stardoc"
)

// A type which allows us to collect multiple proto files
type protoList []string

type docGenFlags struct {
	outDir   string
	zipFile  string
	protos   protoList
	basePath string
}

func (pl *protoList) String() string {
	return fmt.Sprint(*pl)
}

func (pl *protoList) Set(value string) error {
	*pl = append(*pl, value)
	return nil
}

func parseFlags() docGenFlags {
	var flags docGenFlags
	flag.StringVar(&flags.basePath, "base_path", "", "The base path for the TOC.")
	flag.StringVar(&flags.outDir, "out_dir", "", "path to a directory which will contain output files.")
	flag.StringVar(&flags.zipFile, "zip_file", "", "path to a file containing the zip file.")
	flag.Var(&flags.protos, "proto", "path to a protobuf, as a textproto, file which contains the docs")
	flag.Parse()

	return flags
}

func unmarshalProto(proto string) pb.ModuleInfo {
	bytes, err := os.ReadFile(proto)
	if err != nil {
		log.Fatalln("Error reading file:", err)
	}

	var root pb.ModuleInfo
	if err := prototext.Unmarshal(bytes, &root); err != nil {
		log.Fatalln("Failed to parse module info:", err)
	}

	return root
}

func main() {
	flags := parseFlags()

	renderer := bazel_docgen.NewMarkdownRenderer()

	var fileProvider bazel_docgen.FileProvider

	if flags.basePath == "" {
		log.Fatalln("--base_path is a required path")
	}

	if flags.outDir != "" {
		if flags.zipFile != "" {
			log.Fatalln("--zip_file must not be set when using --out_dir.")
		}
		fp := bazel_docgen.NewDirectoryFileProvider(flags.outDir)
		fileProvider = &fp
	} else if flags.zipFile != "" {
		fp := bazel_docgen.NewZipFileProvider(flags.zipFile)
		fileProvider = &fp
	} else {
		log.Fatalln("Either --zip_file or --out_dir must be set.")
	}

	var roots []pb.ModuleInfo

	for _, proto := range flags.protos {
		roots = append(roots, unmarshalProto(proto))
	}

	bazel_docgen.RenderModuleInfo(roots, renderer, fileProvider, flags.basePath)
}
