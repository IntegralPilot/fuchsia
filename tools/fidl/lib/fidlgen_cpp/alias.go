// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type Alias struct {
	Attributes
	fidlgen.Resourceness
	nameVariants
	Type nameVariants
}

func (*Alias) Kind() declKind {
	return Kinds.Alias
}

var _ Kinded = (*Alias)(nil)
var _ namespaced = (*Alias)(nil)

func (c *compiler) compileAlias(val fidlgen.Alias) *Alias {
	name := c.compileNameVariants(val.Name)
	t := c.compileType(val.Type, val.MaybeFromAlias)
	r := Alias{
		Attributes:   Attributes{val.Attributes},
		Resourceness: fidlgen.Resourceness(t.IsResource),
		nameVariants: name,
		Type:         t.nameVariants,
	}
	return &r
}
