// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// [`Depfile`] tracks all the dynamic inputs and outputs of a tool.
///
/// The source of truth of the depfile syntax for our build is the Ninja project,
/// which is somewhat scant on documentation, but from [1] we know it should support
///
/// ```
/// output_1 [output_2 ...]: input_1 [input_2 ...]
/// ```
///
/// Therefore, we can simply remember all the inputs and all the outputs, then
/// concatenate them into a single line in this style. Whenever any input changes,
/// that will cause ninja to re-run our tool which will then dynamically redeclare
/// its outputs. It's not necessary to track on a per-output basis.
///
/// [1]: https://github.com/ninja-build/ninja/blob/ff4f2a0db21b738bba743ad543d8553417aca7b0/src/depfile_parser_test.cc#L222
#[derive(Default)]
pub struct Depfile {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

impl Depfile {
    pub fn new() -> Depfile {
        Depfile::default()
    }

    /// Declare that the tool reads these string paths.
    pub fn track_inputs<I: IntoIterator<Item = String>>(&mut self, iter: I) {
        self.inputs.extend(iter)
    }

    /// Declare that the tool reads this string path.
    pub fn track_input(&mut self, input: String) {
        self.inputs.push(input)
    }

    /// Declare that the tool writes these string paths.
    pub fn track_outputs<I: IntoIterator<Item = String>>(&mut self, iter: I) {
        self.outputs.extend(iter)
    }

    /// Declare that the tool writes this string path.
    pub fn track_output(&mut self, output: String) {
        self.outputs.push(output)
    }
}

impl From<Depfile> for String {
    fn from(value: Depfile) -> Self {
        let inputs = value.inputs.join(" ");
        let outputs = value.outputs.join(" ");
        format!("{outputs}: {inputs}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_depfile() {
        let mut depfile = Depfile::new();
        depfile.track_inputs(["a".to_string()]);
        depfile.track_input("b".to_string());
        depfile.track_outputs(["c".to_string()]);
        depfile.track_output("d".to_string());
        let depfile: String = depfile.into();
        assert_eq!(depfile, "c d: a b");
    }
}
