//! The status operand of `exit` and `return`.

/// The status operand `exit` and `return` were given, read as bash reads it.
pub(crate) enum StatusOperand {
    /// None was given.
    None,
    /// A number, taken modulo 256.
    Status(u8),
    /// One that is not a number (reported).
    NotANumber,
    /// More than one (reported).
    TooMany,
}

/// Reads the status operand of `exit` or `return`, reporting it in bash's words when it is not a
/// number or there is more than one.
pub(crate) fn status_operand(
    context: &brush_core::ExecutionContext<'_, impl brush_core::ShellExtensions>,
    operands: &[String],
) -> Result<StatusOperand, brush_core::Error> {
    match operands {
        [] => Ok(StatusOperand::None),
        [operand] => {
            if let Ok(code) = operand.trim().parse::<i64>() {
                // The status is the number modulo 256.
                Ok(StatusOperand::Status(code.to_le_bytes()[0]))
            } else {
                context.report(format_args!("{operand}: numeric argument required"))?;
                Ok(StatusOperand::NotANumber)
            }
        }
        _ => {
            context.report("too many arguments")?;
            Ok(StatusOperand::TooMany)
        }
    }
}
