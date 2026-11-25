use clap::Parser;

use brush_core::{ExecutionControlFlow, ExecutionResult, builtins};

use crate::status_operand::{StatusOperand, status_operand};

/// Exit the shell.
#[derive(Parser)]
pub(crate) struct ExitCommand {
    /// The exit code to return.
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    code: Vec<String>,
}

impl builtins::Command for ExitCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<brush_core::ExecutionResult, Self::Error> {
        let code_8bit = match status_operand(&context, &self.code)? {
            StatusOperand::Status(code) => code,
            StatusOperand::None => context.shell.last_exit_status(),
            // As in bash: a status that is not a number fails the command; more than one
            // operand still ends the shell, with status 1.
            StatusOperand::NotANumber => return Ok(ExecutionResult::new(2)),
            StatusOperand::TooMany => 1,
        };

        let mut result = ExecutionResult::new(code_8bit);
        result.next_control_flow = ExecutionControlFlow::ExitShell;

        Ok(result)
    }
}

/// Exit a login shell.
#[derive(Parser)]
pub(crate) struct LogoutCommand {
    /// The exit code to return.
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    code: Vec<String>,
}

impl builtins::Command for LogoutCommand {
    type Error = brush_core::Error;

    async fn execute<SE: brush_core::ShellExtensions>(
        &self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<brush_core::ExecutionResult, Self::Error> {
        // As bash words it, `logout` only ends a login shell.
        if !context.shell.options().login_shell {
            context.report("not login shell: use `exit'")?;
            return Ok(ExecutionResult::general_error());
        }
        let code_8bit = match status_operand(&context, &self.code)? {
            StatusOperand::Status(code) => code,
            StatusOperand::None => context.shell.last_exit_status(),
            StatusOperand::NotANumber => return Ok(ExecutionResult::new(2)),
            StatusOperand::TooMany => 1,
        };
        let mut result = ExecutionResult::new(code_8bit);
        result.next_control_flow = ExecutionControlFlow::ExitShell;
        Ok(result)
    }
}
