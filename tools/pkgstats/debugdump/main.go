// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"debug/dwarf"
	"debug/elf"
	"encoding/json"
	"fmt"
	"os"
	"sort"
)

type outputData struct {
	Status string   `json:"status"`
	Err    string   `json:"error"`
	Files  []string `json:"files"`
}

func printJson(data *outputData) {
	val, err := json.Marshal(*data)
	check(err)

	fmt.Println(string(val))
}

func check(e error) {
	if e != nil {
		out := outputData{"FAILURE", e.Error(), make([]string, 0)}
		val, err := json.Marshal(out)
		if err != nil {
			panic(err)
		}
		fmt.Println(string(val))
		os.Exit(-1)
	}
}

func main() {
	if len(os.Args) < 2 || os.Args[1] == "--help" || os.Args[1] == "-h" {
		fmt.Printf("Usage: %s PATH\n\n Print debug info from ELF file at PATH.\n\n -h --help: Show this help and exit.\n", os.Args[0])
		os.Exit(-1)
	}

	f, err := elf.Open(os.Args[1])
	check(err)

	d, err := f.DWARF()
	check(err)

	reader := d.Reader()

	all_files := map[string]bool{}
	for {
		v, err := reader.Next()
		check(err)
		if v == nil {
			break
		}

		if v.Tag == dwarf.TagCompileUnit {
			lr, err := d.LineReader(v)
			check(err)

			entry := dwarf.LineEntry{}

			for lr.Next(&entry) == nil {
				all_files[entry.File.Name] = true
			}
		}
	}

	output_keys := make([]string, len(all_files))
	i := 0
	for k := range all_files {
		output_keys[i] = k
		i += 1
	}

	sort.Strings(output_keys)
	printJson(&outputData{"OK", "", output_keys})
}
