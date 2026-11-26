use crate::{ReadResult, ShellError};

pub(crate) struct NonTermLineReader;

impl super::LineReader for NonTermLineReader {
    fn read_line(
        &self,
        _prompt: Option<&str>,
        _completion_handler: impl FnMut(
            &str,
            usize,
        ) -> Result<
            brush_core::completion::Completions,
            crate::ShellError,
        >,
    ) -> Result<crate::ReadResult, crate::ShellError> {
        // Input is bytes, kept as the shell keeps bytes that are not UTF-8.
        let mut input = Vec::new();
        let bytes_read =
            std::io::BufRead::read_until(&mut std::io::stdin().lock(), b'\n', &mut input)
                .map_err(ShellError::InputError)?;

        if bytes_read == 0 {
            Ok(ReadResult::Eof)
        } else {
            Ok(ReadResult::Input(brush_core::rawbytes::decode_vec(input)))
        }
    }
}
