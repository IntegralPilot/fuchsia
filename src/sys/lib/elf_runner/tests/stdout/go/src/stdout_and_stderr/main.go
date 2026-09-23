// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import "fmt"
import "os"

func main() {
	fmt.Fprintln(os.Stdout, "Hello Stdout!")
	fmt.Fprintln(os.Stderr, "Hello Stderr!")
}
