// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"fmt"
	"io"
	"regexp"
	"sort"
	"strings"
)

const (
	pageTitle = "GN Build Arguments"
	nameDepth = 3
)

var (
	linkRegexp = regexp.MustCompile("//([/A-Za-z-_]+)([.][/A-Za-z-_]+)?")
)

// writeArg emits the name, comment description, and value(s) of the argument in Markdown.
func writeArgs(args []Arg, out io.Writer) {
	if len(args) == 0 {
		return
	}
	sort.Slice(args, func(i, j int) bool {
		return args[i].Name < args[j].Name
	})

	// Include a blank line after heading for properly formatted markdown.
	fmt.Fprintf(out, "%s %s\n\n", strings.Repeat("#", nameDepth), args[0].Name)
	// TODO (juliehockett): Make sure that *all* comments get emitted.
	writeLinkifiedComment(&args[0], out)
	writeAllValues(args, out)
}

// writeValue emits the value of a given argument value, along with the associated Markdown link to its declaration and build (if present).
func writeValue(a *argValue, out io.Writer) {
	var value string
	if strings.Contains(a.Val, "\n") {
		value = fmt.Sprintf("\n\n```none\n%s\n```", a.Val)
	} else {
		value = fmt.Sprintf(" `%s`", a.Val)
	}

	if a.File == "" {
		// If there is no declaration file, emit just the value.
		fmt.Fprintf(out, "%s\n\n", value)
	} else {
		fmt.Fprintf(out, "%s\n\nFrom %s:%d\n\n", value, a.File, a.Line)
	}
}

func writeLinkifiedComment(a *Arg, out io.Writer) {
	if a.Comment != "" {
		fmt.Fprintf(out, "%s\n", a.Comment)
	}
}

func writeAllValues(args []Arg, out io.Writer) {
	emptyArgValue := argValue{}
	for _, a := range args {
		if a.CurrentVal == emptyArgValue || a.CurrentVal == a.DefaultVal {
			fmt.Fprintf(out, "**Current value (from the default):**")
			writeValue(&a.DefaultVal, out)
			return
		}
		fmt.Fprintf(out, "**Current value for `%s`:**", a.Key)
		writeValue(&a.CurrentVal, out)
		fmt.Fprintf(out, "**Overridden from the default:**")
		writeValue(&a.DefaultVal, out)
	}
}
