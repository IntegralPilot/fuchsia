// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::MlsLevel;
use super::security_context::SecurityContext;

use crate::new_policy::traits::PolicyId;
use crate::new_policy::{
    ConstraintOperator, ConstraintSubject, ConstraintTerm, MlsOperands, MlsOperator, NameExpression,
};

/// Evaluates constraint expression for the given source and target [`SecurityContext`]s.
///
/// Assumes that the terms of the constraint expression were sequenced in postfix
/// order and validated at policy load time by [`crate::new_policy::ConstraintNode::validate`].
pub(super) fn evaluate_constraint(
    constraint_expr: &[ConstraintTerm],
    source: &SecurityContext,
    target: &SecurityContext,
) -> bool {
    let mut stack: Vec<bool> = Vec::with_capacity(constraint_expr.len());
    for term in constraint_expr {
        match term {
            ConstraintTerm::Not => {
                let arg = stack.last_mut().expect("validated term sequence");
                *arg = !*arg;
            }
            ConstraintTerm::And => {
                let right = stack.pop().expect("validated term sequence");
                let left = stack.last_mut().expect("validated term sequence");
                *left = *left && right;
            }
            ConstraintTerm::Or => {
                let right = stack.pop().expect("validated term sequence");
                let left = stack.last_mut().expect("validated term sequence");
                *left = *left || right;
            }
            ConstraintTerm::UserAttributeOp(operator) => {
                stack.push(evaluate_simple(source.user(), target.user(), *operator));
            }
            ConstraintTerm::RoleAttributeOp(operator) => {
                stack.push(evaluate_simple(source.role(), target.role(), *operator));
            }
            ConstraintTerm::TypeAttributeOp(operator) => {
                stack.push(evaluate_simple(source.type_(), target.type_(), *operator));
            }
            ConstraintTerm::MlsOp(operand, operator) => {
                let (left_val, right_val) = match operand {
                    MlsOperands::L1L2 => (source.low_level(), target.low_level()),
                    MlsOperands::L1H2 => (source.low_level(), target.effective_high_level()),
                    MlsOperands::H1L2 => (source.effective_high_level(), target.low_level()),
                    MlsOperands::H1H2 => {
                        (source.effective_high_level(), target.effective_high_level())
                    }
                    MlsOperands::L1H1 => (source.low_level(), source.effective_high_level()),
                    MlsOperands::L2H2 => (target.low_level(), target.effective_high_level()),
                };
                stack.push(evaluate_levels(left_val, right_val, *operator));
            }
            ConstraintTerm::UserNameOp(expr) => {
                stack.push(evaluate_name_expr(expr, source.user(), target.user()))
            }
            ConstraintTerm::RoleNameOp(expr) => {
                stack.push(evaluate_name_expr(expr, source.role(), target.role()))
            }
            ConstraintTerm::TypeNameOp(expr) => {
                stack.push(evaluate_name_expr(expr, source.type_(), target.type_()))
            }
        }
    }
    let result = stack.pop().expect("validated term sequence");
    debug_assert!(stack.is_empty(), "validated term sequence leaves stack empty");
    result
}

fn evaluate_simple<T: Eq>(left: T, right: T, operator: ConstraintOperator) -> bool {
    match operator {
        ConstraintOperator::Eq => left == right,
        ConstraintOperator::Ne => left != right,
    }
}

fn evaluate_contains(contains: bool, operator: ConstraintOperator) -> bool {
    match operator {
        ConstraintOperator::Eq => contains,
        ConstraintOperator::Ne => !contains,
    }
}

fn evaluate_name_expr<T: Eq + Copy + PolicyId>(
    expr: &NameExpression<T>,
    source_val: T,
    target_val: T,
) -> bool {
    let val = match expr.subject() {
        ConstraintSubject::Source => source_val,
        ConstraintSubject::Target => target_val,
    };
    evaluate_contains(expr.names().contains(val), expr.operator())
}

fn evaluate_levels(left: &MlsLevel, right: &MlsLevel, operator: MlsOperator) -> bool {
    match operator {
        MlsOperator::Eq => left == right,
        MlsOperator::Ne => left != right,
        MlsOperator::Dom => left.dominates(right),
        MlsOperator::DomBy => right.dominates(left),
        MlsOperator::Incomp => !left.dominates(right) && !right.dominates(left),
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse_policy_by_value;
    use super::*;

    #[test]
    fn evaluate_constraint_expr() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/constraints_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec())
            .expect("parse policy")
            .validate()
            .expect("validate policy");

        let source = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"user1:object_r:security_t:s0:c0-s0:c0".into())
            .expect("valid target security context");

        let classes = policy.classes();
        let class_constraint_eq =
            classes.get_by_name(b"class_constraint_eq").expect("look up class");
        let class_constraint_eq_constraints = class_constraint_eq.constraints();
        assert_eq!(class_constraint_eq_constraints.len(), 1);
        // ( u1 == u2 )
        let constraint_eq = class_constraint_eq_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_eq, &source, &target), false);

        let class_constraint_with_and =
            classes.get_by_name(b"class_constraint_with_and").expect("look up class");
        let class_constraint_with_and_constraints = class_constraint_with_and.constraints();
        assert_eq!(class_constraint_with_and_constraints.len(), 1);
        // ( ( u1 == u2 ) and ( t1 == t2 ) )
        let constraint_with_and = class_constraint_with_and_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_with_and, &source, &target), false);

        let class_constraint_with_not =
            classes.get_by_name(b"class_constraint_with_not").expect("look up class");
        let class_constraint_with_not_constraints = class_constraint_with_not.constraints();
        assert_eq!(class_constraint_with_not_constraints.len(), 1);
        // ( not ( ( u1 == u2 ) and ( t1 == t2 ) )
        let constraint_with_not = class_constraint_with_not_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_with_not, &source, &target), true);

        let class_constraint_with_names =
            classes.get_by_name(b"class_constraint_with_names").expect("look up class");
        let class_constraint_with_names_constraints = class_constraint_with_names.constraints();
        assert_eq!(class_constraint_with_names_constraints.len(), 1);
        // ( u1 != { user0 user1 })
        let constraint_with_names = class_constraint_with_names_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_with_names, &source, &target), false);

        let class_constraint_nested =
            classes.get_by_name(b"class_constraint_nested").expect("look up class");
        let class_constraint_nested_constraints = class_constraint_nested.constraints();
        assert_eq!(class_constraint_nested_constraints.len(), 1);
        // ( ( ( u2 == { user0 user1} ) and ( r1 == r2 ) ) or ( ( u1 == u2 ) and ( not (t1 == t2 ) ) ) )
        let constraint_nested = class_constraint_nested_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_nested, &source, &target), true);

        let class_constraint_role_names =
            classes.get_by_name(b"class_constraint_role_names").expect("look up class");
        let class_constraint_role_names_constraints = class_constraint_role_names.constraints();
        assert_eq!(class_constraint_role_names_constraints.len(), 1);
        // ( r1 == { object_r } ) where source is object_r
        let constraint_role_names = class_constraint_role_names_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_role_names, &source, &target), true);

        let class_constraint_type_names =
            classes.get_by_name(b"class_constraint_type_names").expect("look up class");
        let class_constraint_type_names_constraints = class_constraint_type_names.constraints();
        assert_eq!(class_constraint_type_names_constraints.len(), 1);
        // ( t1 == { domain } ) where source type0 is in domain
        let constraint_type_names = class_constraint_type_names_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_type_names, &source, &target), true);

        let class_constraint_type_list =
            classes.get_by_name(b"class_constraint_type_list").expect("look up class");
        let class_constraint_type_list_constraints = class_constraint_type_list.constraints();
        assert_eq!(class_constraint_type_list_constraints.len(), 1);
        // ( t1 == { type0 security_t } ) where source type is type0
        let constraint_type_list = class_constraint_type_list_constraints[0].constraint_expr();
        assert_eq!(evaluate_constraint(constraint_type_list, &source, &target), true);
    }

    #[test]
    fn evaluate_mls_constraint_expr() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/constraints_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec())
            .expect("parse policy")
            .validate()
            .expect("validate policy");

        let source = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"user1:object_r:security_t:s0:c0-s0:c0".into())
            .expect("valid target security context");

        let classes = policy.classes();
        let class = classes.get_by_name(b"class_mls_constraints").expect("look up class");
        let constraints = class.constraints();
        // Constraints appear in reverse order in parsed policy.
        let expected = vec![
            false, // l1 incomp h1
            false, // h1 incomp h2
            true,  // l1 domby h2
            false, // h1 dom l2
            false, // l2 != h2
            false, // l1 == l2
        ];
        for (i, constraint) in constraints.iter().enumerate() {
            assert_eq!(
                evaluate_constraint(constraint.constraint_expr(), &source, &target),
                expected[i],
                "constraint {}",
                i
            );
        }
    }
}
