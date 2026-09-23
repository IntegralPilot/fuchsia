## BUILD.bazel Header Template
This is the header template for a newly created `BUILD.bazel` file. Use the current year for the copyright year, and ensure `package(default_applicable_licenses = ["//:license"])` is placed after any `load(...)` statements.

```bazel
# Copyright {current_year} The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# load(...) statements go here.

package(default_applicable_licenses = ["//:license"])
```