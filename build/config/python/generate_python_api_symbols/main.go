// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"os/exec"
	"strings"
)

var (
	pythonExe      = flag.String("python-exe", "REQUIRED", "Path to the Python interpreter executable.")
	llvmReadelfExe = flag.String("llvm-readelf-exe", "REQUIRED", "Path to the llvm-readelf executable.")
	outputFile     = flag.String("output", "REQUIRED", "Path to the output file where PROVIDE statements will be written.")
)

func mustExecutable(exePath string, errorMsg string) string {
	if exePath == "REQUIRED" {
		log.Fatalf("Error: %s", errorMsg)
	}
	if _, err := os.Stat(exePath); err != nil {
		if os.IsNotExist(err) {
			log.Fatalf("Error: %s not found at '%s'", errorMsg, exePath)
		} else {
			log.Fatalf("Error: Failed to stat '%s': %v", exePath, err)
		}
	}
	return exePath
}

type ReadelfJson []struct {
	DynamicSymbols []struct {
		Symbol struct {
			Name struct {
				Name string
			}
			Binding struct {
				Name string
			}
		}
	}
}

func main() {
	flag.Parse()

	pythonExe := mustExecutable(*pythonExe, "--python-exe is required")
	llvmReadelfExe := mustExecutable(*llvmReadelfExe, "--llvm-readelf-exe is required")

	if *outputFile == "REQUIRED" {
		log.Fatal("Error: --output is required")
	}
	outputFile := *outputFile

	cmd := exec.Command(llvmReadelfExe, "--dyn-syms", "--elf-output-style=JSON", pythonExe)
	cmd.Stderr = os.Stderr
	readelfOut, err := cmd.Output()
	if err != nil {
		log.Fatalf("Error running llvm-readelf: %v", err)
	}

	var readelfOutJson ReadelfJson
	err = json.Unmarshal(readelfOut, &readelfOutJson)
	if err != nil {
		log.Fatalf("Error unmarshaling JSON: %v", err)
	}

	// Write PROVIDE statements to the output file by transforming each Python C API symbol into
	// a corresponding linker script function to provide an equivalent weak symbol.
	//
	// The JSON from llvm-readelf looks like the following:
	//
	// [
	//		{
	//			...,
	//			"DynamicSymbols": [
	//				{
	//					"Symbol": {
	//						"Name": {
	//							"Name": "PyCapsule_Type",
	//							...
	//						},
	//						"Binding": {
	//							"Name": "Global",
	//							...
	//						},
	//					...
	//					}
	//				},
	//				...
	//			]
	//		},
	//     ...
	// ]
	var output []string
	for _, entry := range readelfOutJson {
		for _, symbol := range entry.DynamicSymbols {
			if symbol.Symbol.Binding.Name == "Global" &&
				(strings.HasPrefix(symbol.Symbol.Name.Name, "_Py") ||
					strings.HasPrefix(symbol.Symbol.Name.Name, "Py")) {
				output = append(output, fmt.Sprintf("PROVIDE(%s = 0);", symbol.Symbol.Name.Name))
			}
		}
	}

	err = os.WriteFile(outputFile, []byte(strings.Join(output, "\n")), 0644)
	if err != nil {
		log.Fatalf("Error writing to output file '%s': %v", outputFile, err)
	}
}
