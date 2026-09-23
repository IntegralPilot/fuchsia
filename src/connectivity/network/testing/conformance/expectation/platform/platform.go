// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package platform

import "fmt"

type Platform int

const (
	_ Platform = iota
	NS2
	NS3
)

func (p Platform) String() string {
	switch p {
	case NS2:
		return "NS2"
	case NS3:
		return "NS3"
	default:
		panic(fmt.Sprintf("Unrecognized Platform: %d", p))
	}
}

func (p Platform) FfxFlag() string {
	switch p {
	case NS2:
		return "v2"
	case NS3:
		return "v3"
	default:
		panic(fmt.Sprintf("Unrecognized Platform: %d", p))
	}
}
