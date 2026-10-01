//! Parser for shell test commands.

use crate::{ast, error};

/// Parses a test command expression.
///
/// # Arguments
///
/// * `input` - The test command expression to parse, in string form.
pub fn parse<S: AsRef<str>>(input: &[S]) -> Result<ast::TestExpr, error::TestCommandParseError> {
    let args: Vec<&str> = input.iter().map(AsRef::as_ref).collect();
    // Up to four arguments, their number decides how they are read, as POSIX specifies.
    if let Ok(expr) = test_command::short_expression(&args) {
        return Ok(expr);
    }
    if let Some(message) = short_form_error(&args) {
        return Err(error::TestCommandParseError::Syntax(message));
    }
    let mut expression = Expression {
        args: &args,
        position: 0,
        depth: 0,
    };
    let expr = expression.or()?;
    match expression.peek(0) {
        None => Ok(expr),
        Some(extra) if extra.starts_with('-') => {
            Err(syntax(std::format!("syntax error: `{extra}' unexpected")))
        }
        Some(_) => Err(syntax("too many arguments")),
    }
}

/// Why arguments that bash reads by their number are no test expression, as bash words it, or
/// `None` when bash reads them as a general expression.
fn short_form_error(args: &[&str]) -> Option<String> {
    match args {
        [first, _] | ["(", first, _, ")"] => Some(std::format!("{first}: unary operator expected")),
        ["!", second, _] => Some(std::format!("{second}: unary operator expected")),
        [_, second, _] => Some(std::format!("{second}: binary operator expected")),
        ["!", rest @ ..] if rest.len() == 3 => short_form_error(rest),
        _ => None,
    }
}

fn syntax(message: impl Into<String>) -> error::TestCommandParseError {
    error::TestCommandParseError::Syntax(message.into())
}

/// How deeply parentheses may nest in a test expression. Reading and evaluating it recurse once
/// per level; bash's limit is its much larger stack.
const MAX_NESTING: usize = 64;

/// Reads arguments as bash's general test expression parser does: `-o` joins lists joined by
/// `-a` of terms, and a term is negated by any number of `!` and may be a parenthesized
/// expression. Only the parentheses nest; lists and `!` runs of any length are read in loops.
struct Expression<'a> {
    args: &'a [&'a str],
    position: usize,
    depth: usize,
}

impl<'a> Expression<'a> {
    fn peek(&self, offset: usize) -> Option<&'a str> {
        self.args.get(self.position + offset).copied()
    }

    fn or(&mut self) -> Result<ast::TestExpr, error::TestCommandParseError> {
        let mut operands = vec![self.and()?];
        while self.peek(0) == Some("-o") {
            self.position += 1;
            operands.push(self.and()?);
        }
        Ok(joined(operands, ast::TestExpr::Or))
    }

    fn and(&mut self) -> Result<ast::TestExpr, error::TestCommandParseError> {
        let mut operands = vec![self.term()?];
        while self.peek(0) == Some("-a") {
            self.position += 1;
            operands.push(self.term()?);
        }
        Ok(joined(operands, ast::TestExpr::And))
    }

    fn term(&mut self) -> Result<ast::TestExpr, error::TestCommandParseError> {
        let argument_expected = || syntax("argument expected");
        let mut negated = false;
        while self.peek(0) == Some("!") {
            self.position += 1;
            negated = !negated;
        }
        let first = self.peek(0).ok_or_else(argument_expected)?;
        let term = if first == "(" {
            if self.depth >= MAX_NESTING {
                return Err(error::TestCommandParseError::NestedTooDeeply);
            }
            self.position += 1;
            self.depth += 1;
            let inner = self.or()?;
            self.depth -= 1;
            match self.peek(0) {
                Some(")") => self.position += 1,
                Some(found) => return Err(syntax(std::format!("`)' expected, found {found}"))),
                None => return Err(syntax("`)' expected")),
            }
            ast::TestExpr::Parenthesized(Box::new(inner))
        } else if let (Some(op), Some(right)) =
            (self.peek(1).and_then(binary_predicate), self.peek(2))
        {
            self.position += 3;
            ast::TestExpr::BinaryTest(op, first.to_owned(), right.to_owned())
        } else if let (Some(op), Some(operand)) = (unary_predicate(first), self.peek(1)) {
            self.position += 2;
            ast::TestExpr::UnaryTest(op, operand.to_owned())
        } else {
            self.position += 1;
            ast::TestExpr::Literal(first.to_owned())
        };
        Ok(if negated {
            ast::TestExpr::Not(Box::new(term))
        } else {
            term
        })
    }
}

/// The operands joined by `join`, or the only one.
fn joined(
    operands: Vec<ast::TestExpr>,
    join: fn(Vec<ast::TestExpr>) -> ast::TestExpr,
) -> ast::TestExpr {
    match <[ast::TestExpr; 1]>::try_from(operands) {
        Ok([only]) => only,
        Err(operands) => join(operands),
    }
}

fn unary_predicate(op: &str) -> Option<ast::UnaryPredicate> {
    Some(match op {
        "-a" | "-e" => ast::UnaryPredicate::FileExists,
        "-b" => ast::UnaryPredicate::FileExistsAndIsBlockSpecialFile,
        "-c" => ast::UnaryPredicate::FileExistsAndIsCharSpecialFile,
        "-d" => ast::UnaryPredicate::FileExistsAndIsDir,
        "-f" => ast::UnaryPredicate::FileExistsAndIsRegularFile,
        "-g" => ast::UnaryPredicate::FileExistsAndIsSetgid,
        "-h" | "-L" => ast::UnaryPredicate::FileExistsAndIsSymlink,
        "-k" => ast::UnaryPredicate::FileExistsAndHasStickyBit,
        "-n" => ast::UnaryPredicate::StringHasNonZeroLength,
        "-o" => ast::UnaryPredicate::ShellOptionEnabled,
        "-p" => ast::UnaryPredicate::FileExistsAndIsFifo,
        "-r" => ast::UnaryPredicate::FileExistsAndIsReadable,
        "-s" => ast::UnaryPredicate::FileExistsAndIsNotZeroLength,
        "-t" => ast::UnaryPredicate::FdIsOpenTerminal,
        "-u" => ast::UnaryPredicate::FileExistsAndIsSetuid,
        "-v" => ast::UnaryPredicate::ShellVariableIsSetAndAssigned,
        "-w" => ast::UnaryPredicate::FileExistsAndIsWritable,
        "-x" => ast::UnaryPredicate::FileExistsAndIsExecutable,
        "-z" => ast::UnaryPredicate::StringHasZeroLength,
        "-G" => ast::UnaryPredicate::FileExistsAndOwnedByEffectiveGroupId,
        "-N" => ast::UnaryPredicate::FileExistsAndModifiedSinceLastRead,
        "-O" => ast::UnaryPredicate::FileExistsAndOwnedByEffectiveUserId,
        "-R" => ast::UnaryPredicate::ShellVariableIsSetAndNameRef,
        "-S" => ast::UnaryPredicate::FileExistsAndIsSocket,
        _ => return None,
    })
}

fn binary_predicate(op: &str) -> Option<ast::BinaryPredicate> {
    Some(match op {
        "=" | "==" => ast::BinaryPredicate::StringExactlyMatchesString,
        "-ef" => ast::BinaryPredicate::FilesReferToSameDeviceAndInodeNumbers,
        "-eq" => ast::BinaryPredicate::ArithmeticEqualTo,
        "-ge" => ast::BinaryPredicate::ArithmeticGreaterThanOrEqualTo,
        "-gt" => ast::BinaryPredicate::ArithmeticGreaterThan,
        "-le" => ast::BinaryPredicate::ArithmeticLessThanOrEqualTo,
        "-lt" => ast::BinaryPredicate::ArithmeticLessThan,
        "-ne" => ast::BinaryPredicate::ArithmeticNotEqualTo,
        "-nt" => ast::BinaryPredicate::LeftFileIsNewerOrExistsWhenRightDoesNot,
        "-ot" => ast::BinaryPredicate::LeftFileIsOlderOrDoesNotExistWhenRightDoes,
        "!=" => ast::BinaryPredicate::StringDoesNotExactlyMatchString,
        "<" => ast::BinaryPredicate::LeftSortsBeforeRight,
        ">" => ast::BinaryPredicate::LeftSortsAfterRight,
        _ => return None,
    })
}

peg::parser! {
    grammar test_command<'a>() for [&'a str] {
        pub(crate) rule short_expression() -> ast::TestExpr =
            end() { ast::TestExpr::False } /
            e:one_arg_expr() end() { e } /
            e:two_arg_expr() end()  { e } /
            e:three_arg_expr() end()  { e } /
            e:four_arg_expr() end()  { e }

        rule one_arg_expr() -> ast::TestExpr =
            [s] { ast::TestExpr::Literal(s.to_owned()) }

        rule two_arg_expr() -> ast::TestExpr =
            ["!"] e:one_arg_expr() { ast::TestExpr::Not(Box::from(e)) } /
            op:unary_op() [s] { ast::TestExpr::UnaryTest(op, s.to_owned()) }

        rule three_arg_expr() -> ast::TestExpr =
            [left] ["-a"] [right] { ast::TestExpr::And(vec![ast::TestExpr::Literal(left.to_owned()), ast::TestExpr::Literal(right.to_owned())]) } /
            [left] ["-o"] [right] { ast::TestExpr::Or(vec![ast::TestExpr::Literal(left.to_owned()), ast::TestExpr::Literal(right.to_owned())]) } /
            [left] op:binary_op() [right] { ast::TestExpr::BinaryTest(op, left.to_owned(), right.to_owned()) } /
            ["!"] e:two_arg_expr() { ast::TestExpr::Not(Box::from(e)) } /
            ["("] e:one_arg_expr() [")"] { e }

        rule four_arg_expr() -> ast::TestExpr =
            ["!"] e:three_arg_expr() { ast::TestExpr::Not(Box::from(e)) } /
            ["("] e:two_arg_expr() [")"] { e }

        rule unary_op() -> ast::UnaryPredicate =
            [s] {? unary_predicate(s).ok_or("unary operator") }

        rule binary_op() -> ast::BinaryPredicate =
            [s] {? binary_predicate(s).ok_or("binary operator") }

        rule end() = ![_]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn and_binds_tighter_than_or() {
        // `x -o '' -a ''` is `x -o ('' -a '')`, as bash reads it.
        let expr = parse(&["x", "-o", "", "-a", ""]).ok();
        assert!(
            matches!(
                &expr,
                Some(ast::TestExpr::Or(operands))
                    if matches!(operands.as_slice(), [ast::TestExpr::Literal(_), ast::TestExpr::And(..)])
            ),
            "{expr:?}"
        );
        let expr = parse(&["a", "-a", "b", "-o", "c", "-a", "d"]).ok();
        assert!(
            matches!(
                &expr,
                Some(ast::TestExpr::Or(operands))
                    if matches!(operands.as_slice(), [ast::TestExpr::And(..), ast::TestExpr::And(..)])
            ),
            "{expr:?}"
        );
    }

    #[test]
    fn long_lists_and_negations_do_not_nest() {
        let mut args = vec!["x"];
        for _ in 0..50_000 {
            args.extend(["-a", "-f", "y", "-o", "z"]);
        }
        let expr = parse(&args).ok();
        assert!(
            matches!(&expr, Some(ast::TestExpr::Or(operands)) if operands.len() == 50_001),
            "{:?}",
            expr.map(|expr| expr.to_string().len())
        );

        let mut args = vec!["!"; 50_001];
        args.push("x");
        assert!(matches!(
            parse(&args),
            Ok(ast::TestExpr::Not(operand)) if matches!(*operand, ast::TestExpr::Literal(_))
        ));
    }

    #[test]
    fn parentheses_nest_up_to_the_limit() {
        let nested = |depth: usize| {
            let mut args = vec!["("; depth];
            args.extend(["-n", "x"]);
            args.extend(vec![")"; depth]);
            args.extend(["-a", "y"]);
            parse(&args)
        };
        assert!(nested(MAX_NESTING).is_ok());
        assert!(matches!(
            nested(MAX_NESTING + 1),
            Err(error::TestCommandParseError::NestedTooDeeply)
        ));
    }

    #[test]
    fn errors_are_worded_as_bash_words_them() {
        let message = |args: &[&str]| match parse(args) {
            Err(error::TestCommandParseError::Syntax(message)) => message,
            other => std::format!("{other:?}"),
        };
        assert_eq!(message(&["a", "b"]), "a: unary operator expected");
        assert_eq!(message(&["(", "a", "b"]), "a: binary operator expected");
        assert_eq!(message(&["!", "a", "b"]), "a: unary operator expected");
        assert_eq!(message(&["(", "a", "b", ")"]), "a: unary operator expected");
        assert_eq!(
            message(&["!", "a", "b", "c"]),
            "b: binary operator expected"
        );
        assert_eq!(message(&["a", "b", "c", "d"]), "too many arguments");
        assert_eq!(
            message(&["-z", "-o", "-n", "x"]),
            "syntax error: `-n' unexpected"
        );
        assert_eq!(message(&["a", "-a", "b", "-a", "!"]), "argument expected");
        assert_eq!(message(&["a", "-a", "b", "-o"]), "argument expected");
        assert_eq!(message(&["(", "a", "-a", "b"]), "`)' expected");
        assert_eq!(
            message(&["(", "=", "(", "-a", "x"]),
            "`)' expected, found ("
        );
    }

    #[test]
    fn a_term_is_read_as_bash_reads_it() {
        // An opening parenthesis starts a group before it can be an operand, and a missing
        // operand after `!` is an error, not a literal `!`.
        assert!(parse(&["(", "=", "(", "-a", "x"]).is_err());
        assert!(parse(&["a", "-a", "b", "-a", "!"]).is_err());
        assert!(matches!(
            parse(&["!", "!", "-n", "x", "-a", "y"]),
            Ok(ast::TestExpr::And(operands))
                if matches!(operands.as_slice(), [ast::TestExpr::UnaryTest(..), ast::TestExpr::Literal(_)])
        ));
    }
}
