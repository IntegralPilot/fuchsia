// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::super::procedure_manipulated_state::ProcedureManipulatedState;
use super::{Procedure, ProcedureInputT, ProcedureOutputT};

use anyhow::{Result, format_err};
use paste::paste;

#[allow(unused)]
#[derive(Clone, Debug, PartialEq)]
pub enum ProcedureInput {
    StartProcedureOne,
    ContinueProcedureOne,
    TerminateProcedureOne,

    StartProcedureTwo,
    ContinueProcedureTwo,
    TerminateProcedureTwo,

    StartProcedureThree,
    ContinueProcedureThree,
    TerminateProcedureThree,
}

#[allow(unused)]
#[derive(Clone, Debug, PartialEq)]
pub enum ProcedureOutput {
    ProcedureOneStarted,
    ProcedureOneContinued,
    ProcedureOneTerminated,

    ProcedureTwoStarted,
    ProcedureTwoContinued,
    ProcedureTwoTerminated,

    ProcedureThreeStarted,
    ProcedureThreeContinued,
    ProcedureThreeTerminated,
}

impl ProcedureInputT<ProcedureOutput> for ProcedureInput {
    fn to_initialized_procedure(&self) -> Option<Box<dyn Procedure<Self, ProcedureOutput>>> {
        match self {
            ProcedureInput::StartProcedureOne => Some(Box::new(ProcedureOne::new())),
            ProcedureInput::StartProcedureTwo => Some(Box::new(ProcedureTwo::new())),
            ProcedureInput::StartProcedureThree => Some(Box::new(ProcedureThree::new())),
            _ => None,
        }
    }

    fn can_start_procedure(&self) -> bool {
        match self {
            ProcedureInput::StartProcedureOne
            | ProcedureInput::StartProcedureTwo
            | ProcedureInput::StartProcedureThree => true,
            _ => false,
        }
    }
}

impl ProcedureOutputT for ProcedureOutput {}

macro_rules! test_procedure {
    ($name: ident) => {
        paste! {
            #[derive(Copy, Clone, Debug, PartialEq)]
            pub enum [< Procedure $name >] {
                New,
                Started,
                Continued,
                Terminated,
            }

            impl [< Procedure $name >] {
                pub fn new() -> [< Procedure $name >] {
                    Self::New
                }
            }

            impl Procedure<ProcedureInput, ProcedureOutput> for [< Procedure $name >] {
                fn name(&self) -> &str {
                    std::any::type_name::<Self>()
                }

                fn transition(
                    &mut self,
                    _state: &mut ProcedureManipulatedState,
                    input: ProcedureInput,
                ) -> Result<Vec<ProcedureOutput>> {
                    match (&self, &input) {
                        (Self::New, ProcedureInput::[<Start Procedure $name >]) => {
                            *self = Self::Started;
                            Ok(vec![ProcedureOutput::[< Procedure $name Started>]])
                        }
                        (Self::Started, ProcedureInput::[<Continue Procedure $name >]) => {
                            *self = Self::Continued;
                            Ok(vec![ProcedureOutput::[< Procedure $name Continued>]])
                        }
                        (Self::Continued, ProcedureInput::[<Terminate Procedure $name >]) => {
                            *self = Self::Terminated;
                            Ok(vec![ProcedureOutput::[< Procedure $name Terminated>]])
                        }
                        _ => Err(format_err!(
                            "Bad transition for procedure {:?}, state {:?}, input {:?}",
                            self.name(),
                            self,
                            input
                        )),
                    }
                }

                fn is_terminated(&self) -> bool {
                    match self {
                        Self::Terminated => true,
                        _ => false,
                    }
                }
            }
        }
    };
}

test_procedure!(One);
test_procedure!(Two);
test_procedure!(Three);
