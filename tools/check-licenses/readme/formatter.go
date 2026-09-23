// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import "go.fuchsia.dev/fuchsia/tools/readme_fuchsia"

func Format(readmes []*Readme) string {
	return readme_fuchsia.Format(readmes)
}
