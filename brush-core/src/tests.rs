//! Shell test conditional expressions

use crate::{ExecutionParameters, Shell, error, extendedtests, extensions};

/// Evaluate the given test expression within the provided shell and
/// execution context. Returns true if the expression evaluates to true,
/// false otherwise.
///
/// # Arguments
///
/// * `expr` - The test expression to evaluate.
/// * `shell` - The shell context in which to evaluate the expression.
/// * `params` - The execution parameters to use during evaluation.
pub fn eval_expr(
    expr: &brush_parser::ast::TestExpr,
    shell: &mut Shell<impl extensions::ShellExtensions>,
    params: &ExecutionParameters,
) -> Result<bool, error::Error> {
    match expr {
        brush_parser::ast::TestExpr::False => Ok(false),
        brush_parser::ast::TestExpr::Literal(s) => Ok(!s.is_empty()),
        // Bash evaluates every operand of `-a` and `-o`, so one that is invalid fails the test
        // even after the others have decided it.
        brush_parser::ast::TestExpr::And(operands) => {
            let mut all = true;
            for operand in operands {
                all &= eval_expr(operand, shell, params)?;
            }
            Ok(all)
        }
        brush_parser::ast::TestExpr::Or(operands) => {
            let mut any = false;
            for operand in operands {
                any |= eval_expr(operand, shell, params)?;
            }
            Ok(any)
        }
        brush_parser::ast::TestExpr::Not(expr) => Ok(!eval_expr(expr, shell, params)?),
        brush_parser::ast::TestExpr::Parenthesized(expr) => eval_expr(expr, shell, params),
        brush_parser::ast::TestExpr::UnaryTest(op, operand) => {
            extendedtests::apply_unary_predicate_to_str(op, operand, shell, params)
        }
        brush_parser::ast::TestExpr::BinaryTest(op, left, right) => {
            extendedtests::apply_binary_predicate_to_strs(op, left.as_str(), right.as_str(), shell)
        }
    }
}
