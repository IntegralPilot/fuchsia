// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This binary takes the output of "fx gn gen" (project.json) and
// saves the resulting gen struct object to disk at the location defined
// by "--gen_output".
package main

import (
	"encoding/json"
	"flag"
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/util"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/util/cmd"
)

var (
	genInput  = flag.String("gen_input", "", "Location of project.json file.")
	genOutput = flag.String("gen_output", "", "Location to save intermediate GN gen output.")
)

func main() {
	var err error
	flag.Parse()

	if *genInput == "" {
		cmd.Exit(fmt.Errorf("--gen_input must be provided."))
	}
	if *genOutput == "" {
		cmd.Exit(fmt.Errorf("--gen_output must be provided."))
	}

	gen, err := util.LoadGen(*genInput)
	if err != nil {
		cmd.Exit(err)
	}

	var data []byte
	data, err = json.MarshalIndent(gen, "", "  ")
	if err != nil {
		cmd.Exit(err)
	}
	cmd.Exit(cmd.SaveFile(data, *genOutput))
}
