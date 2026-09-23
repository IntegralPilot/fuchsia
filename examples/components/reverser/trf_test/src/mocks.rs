// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_examples_reverser as freverser;
use futures::StreamExt;
use std::sync::Mutex;
use trf_codegen_macro as trf;

struct ReplacerRules {
    target: String,
    replacement: String,
}

#[trf::mock(protocol = "fuchsia.examples.reverser.Replacer")]
pub struct MockReplacer {
    rules: Mutex<ReplacerRules>,
}

impl MockReplacer {
    pub fn new() -> Self {
        Self {
            rules: Mutex::new(ReplacerRules { target: String::new(), replacement: String::new() }),
        }
    }

    #[trf::control]
    pub fn set_replacement(&self, target: String, replacement: String) {
        let mut rules = self.rules.lock().unwrap();
        rules.target = target;
        rules.replacement = replacement;
    }

    pub async fn serve_replacer(
        &self,
        mut stream: freverser::ReplacerRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(Ok(req)) = stream.next().await {
            match req {
                freverser::ReplacerRequest::Replace { value, responder } => {
                    let result = {
                        let rules = self.rules.lock().unwrap();
                        if !rules.target.is_empty() {
                            value.replace(&rules.target, &rules.replacement)
                        } else {
                            value
                        }
                    };
                    let _ = responder.send(&result);
                }
            }
        }
        Ok(())
    }
}
