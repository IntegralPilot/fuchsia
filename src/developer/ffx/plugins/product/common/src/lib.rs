// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use ffx_writer::VerifiedMachineWriter;
use fho::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use structured_ui::{Interface, Presentation, Response};

/// CommandStatus is returned to indicate exit status of
/// a command. The Ok variant is optional, and is intended
/// for use with commands that return no other data so there
/// is some indication of correct execution.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok { message: Option<String> },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum MachineOutput<T: JsonSchema + Serialize> {
    CommandStatus(CommandStatus),
    Notice { title: Option<String>, message: Option<String> },
    // Since we are using a tag field to identify the enum variant,
    // T cannot be simple type since there is no field name to associate
    // the value with.
    Data(T),
}

// A structured_ui::Interface implementation for use
// when machine output is needed.
pub struct MachineUi<T: Serialize + JsonSchema> {
    writer: Rc<RefCell<VerifiedMachineWriter<MachineOutput<T>>>>,
}

impl<T: Serialize + JsonSchema> Interface for MachineUi<T> {
    fn present(
        &self,
        output: &structured_ui::Presentation,
    ) -> std::result::Result<Response, structured_ui::StructuredUiError> {
        match output {
            Presentation::Notice(notice) => self
                .machine(MachineOutput::<T>::Notice {
                    title: notice.get_title(),
                    message: notice.get_message(),
                })
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
            Presentation::Progress(_) => (), //ignore progress for machine output.
            Presentation::StringPrompt(p) => {
                todo!("String prompt not supported in machine mode: {p:?}")
            }
            Presentation::Table(_table) => todo!("Table not supported in machine mode: {_table:?}"),
        };

        Ok(Response::Default)
    }
}

impl<T: Serialize + JsonSchema> MachineUi<T> {
    pub fn new(writer: VerifiedMachineWriter<MachineOutput<T>>) -> Self {
        MachineUi { writer: Rc::new(RefCell::new(writer)) }
    }

    pub fn machine(&self, data: MachineOutput<T>) -> Result<()> {
        self.writer.borrow_mut().machine(&data).map_err(move |e| e.into())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_writer::{Format, TestBuffers};

    #[derive(Debug, Serialize, JsonSchema)]
    struct TestBundle {
        pub base_url: String,
        pub value: u32,
        pub name: String,
    }

    impl TestBundle {
        fn new() -> Self {
            Self { base_url: "/some/url".into(), value: 42, name: "Sample_Bundle".into() }
        }
    }

    #[fuchsia::test]
    fn test_machine_schema() {
        let outputs = vec![
            MachineOutput::CommandStatus(CommandStatus::Ok { message: None }),
            MachineOutput::CommandStatus(CommandStatus::Ok {
                message: Some("Ok with message".into()),
            }),
            MachineOutput::CommandStatus(CommandStatus::UnexpectedError {
                message: "an error".into(),
            }),
            MachineOutput::CommandStatus(CommandStatus::UserError { message: "an error".into() }),
            MachineOutput::Notice { title: None, message: None },
            MachineOutput::Notice { title: Some("The title".into()), message: None },
            MachineOutput::Notice {
                title: Some("The title".into()),
                message: Some("a message".into()),
            },
            MachineOutput::Notice { title: None, message: Some("a message".into()) },
            MachineOutput::Data(TestBundle::new()),
        ];

        for o in outputs {
            let test_buffers = TestBuffers::default();
            let mut writer = VerifiedMachineWriter::<MachineOutput<TestBundle>>::new_test(
                Some(Format::JsonPretty),
                &test_buffers,
            );
            writer.machine(&o).expect("write data");
            let data_str = test_buffers.into_stdout_str();
            let data = serde_json::from_str(&data_str).expect("json value");
            match VerifiedMachineWriter::<MachineOutput<TestBundle>>::verify_schema(&data) {
                Ok(_) => (),
                Err(e) => {
                    println!("Error verifying schema: {e}");
                    println!("{data:?}");
                }
            };
        }
    }
    #[fuchsia::test]
    fn test_machine_no_value_schema() {
        let outputs = vec![MachineOutput::Data(())];

        for o in outputs {
            let test_buffers = TestBuffers::default();
            let mut writer = VerifiedMachineWriter::<MachineOutput<()>>::new_test(
                Some(Format::JsonPretty),
                &test_buffers,
            );
            writer.machine(&o).expect("write data");
            let data_str = test_buffers.into_stdout_str();
            let data = serde_json::from_str(&data_str).expect("json value");
            match VerifiedMachineWriter::<MachineOutput<()>>::verify_schema(&data) {
                Ok(_) => (),
                Err(e) => {
                    println!("Error verifying schema: {e}");
                    println!("{data:?}");
                }
            };
        }
    }
}
