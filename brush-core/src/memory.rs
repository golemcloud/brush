//! The memory the shell holds, and the budget it keeps to.
//!
//! Under WASI a failed allocation cannot be recovered from: Rust aborts, and the component traps.
//! So the shell refuses, as bash does when `malloc` fails, what would take the memory it holds past
//! [`MAX_SHELL_MEMORY_BYTES`]: a variable's value, an array's elements, the text of an expansion in
//! flight, a parse, a background job. It checks before it copies or builds such data, where it
//! knows the size, and again where it stores it.
//!
//! What the shell holds is what the C library's allocator has handed out and not taken back, Rust's
//! allocations and the C library's own alike. The embedder counts it, wrapping `malloc` and `free`,
//! with [`allocated`] and [`released`]; without that only the size of each request counts.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error;

/// The limit, in MiB, of [`MAX_SHELL_MEMORY_BYTES`], as a literal so messages can name it.
macro_rules! memory_limit_mib {
    () => {
        384
    };
}

/// The most memory the shell holds at once.
///
/// That is every variable, array element, expansion in flight, parse, subshell, job and buffer of
/// the call. Past it, whatever needs more fails as a failed `malloc` does in bash (see
/// [`reserve`]). It is well below the 1 GiB an agent's linear memory may grow to, which must also
/// hold the allocator's free space, the stack and the copies made between checks.
pub const MAX_SHELL_MEMORY_BYTES: usize = memory_limit_mib!() * 1024 * 1024;

/// What the shell's refusal adds to bash's own `xmalloc` diagnostic.
pub(crate) const OVER_BUDGET: &str = concat!(
    "shell memory over ",
    memory_limit_mib!(),
    " MiB is unsupported in bash-tool"
);

/// About what one element of a list costs besides its text: its string's header, its place in an
/// array's tree or a field's vector, and the allocator's own overhead.
pub const ELEMENT_BYTES: usize = 64;

/// About what one word of a brace expansion costs besides its text while it is expanded: the word,
/// the field it expands to, and the argument it becomes.
pub const WORD_BYTES: usize = 160;

/// About what a background job holds until it is waited for: its copy of the shell's state and
/// its record.
pub const JOB_BYTES: usize = 32 * 1024;

/// Bytes the allocator has handed out and not taken back, as [`allocated`] and [`released`] count
/// them.
static IN_USE: AtomicUsize = AtomicUsize::new(0);

/// Counts `bytes` the allocator handed out.
pub fn allocated(bytes: usize) {
    IN_USE.fetch_add(bytes, Ordering::Relaxed);
}

/// Counts `bytes` the allocator took back.
pub fn released(bytes: usize) {
    IN_USE.fetch_sub(bytes, Ordering::Relaxed);
}

/// The bytes the shell holds now (0 when the embedder does not count them).
pub fn in_use() -> usize {
    IN_USE.load(Ordering::Relaxed)
}

/// About the bytes an allocation of `size` takes from the heap: dlmalloc's 4-byte header, rounded
/// to its 16-byte granule.
pub const fn allocation_bytes(size: usize) -> usize {
    let chunk = size.saturating_add(4 + 15) & !15;
    if chunk < 16 { 16 } else { chunk }
}

/// The bytes a copy of `text` takes.
pub const fn text_bytes(text: &str) -> usize {
    allocation_bytes(text.len())
}

/// Whether the shell can hold `bytes` more.
pub fn can_hold(bytes: usize) -> bool {
    fits(in_use(), bytes)
}

/// Checks that the shell can hold `bytes` more.
///
/// # Errors
///
/// Past [`MAX_SHELL_MEMORY_BYTES`], the error bash's `xmalloc` reports: it ends the shell (or the
/// subshell it happens in) with status 2, as a failed allocation ends bash.
pub fn reserve(bytes: usize) -> Result<(), error::Error> {
    if can_hold(bytes) {
        Ok(())
    } else {
        Err(out_of_memory(bytes))
    }
}

/// Checks that a buffer of `len` bytes, with room for `capacity`, can grow by `additional`: when it
/// must move, that its new allocation, twice the old one, fits.
///
/// # Errors
///
/// As [`reserve`].
pub fn reserve_growth(len: usize, capacity: usize, additional: usize) -> Result<(), error::Error> {
    let needed = len.saturating_add(additional);
    if needed <= capacity {
        Ok(())
    } else {
        reserve(allocation_bytes(needed.max(capacity.saturating_mul(2))))
    }
}

/// Elements added to a list between checks that the shell can hold as many more (see
/// [`reserve_push`]).
const ELEMENTS_PER_CHECK: usize = 1024;

/// Checks that `list` can take one more element.
///
/// The element holds about `element_bytes` besides its place in the list. When the list is full,
/// this checks that its next allocation, twice the old one, fits; and every
/// [`ELEMENTS_PER_CHECK`] elements, that as many more do.
///
/// # Errors
///
/// As [`reserve`].
pub fn reserve_push<T>(list: &Vec<T>, element_bytes: usize) -> Result<(), error::Error> {
    let size = std::mem::size_of::<T>();
    reserve_growth(list.len() * size, list.capacity() * size, size)?;
    if list.len().is_multiple_of(ELEMENTS_PER_CHECK) {
        reserve(ELEMENTS_PER_CHECK.saturating_mul(element_bytes))?;
    }
    Ok(())
}

/// Checks that the shell is still within its budget now that it holds something of `bytes`: a
/// value it stored, an expansion it finished.
///
/// # Errors
///
/// As [`reserve`], naming `bytes`.
pub fn check_held(bytes: usize) -> Result<(), error::Error> {
    if in_use() <= MAX_SHELL_MEMORY_BYTES {
        Ok(())
    } else {
        Err(out_of_memory(bytes))
    }
}

/// The error for a request of `bytes` the shell cannot hold: bash's `xmalloc` failure, which ends
/// the shell.
pub fn out_of_memory(bytes: usize) -> error::Error {
    error::Error::from(error::ErrorKind::OutOfMemory(bytes)).into_fatal()
}

/// About the bytes a parse holds for each token it reads: the token, its share of the syntax
/// tree, and the parser's working state.
const PARSE_TOKEN_BYTES: usize = 512;

/// About the bytes a parse holds for each character of its text: the copies the tokenizer and
/// the shell keep of it, and of a word or here-document body it makes up.
const PARSE_CHAR_BYTES: usize = 24;

/// About the bytes a parse of `tokens` tokens from `chars` characters of text holds.
const fn parse_bytes(tokens: usize, chars: usize) -> usize {
    tokens
        .saturating_mul(PARSE_TOKEN_BYTES)
        .saturating_add(chars.saturating_mul(PARSE_CHAR_BYTES))
}

/// Has every parse keep to the budget: one that cannot hold its syntax tree stops reading, and
/// fails as [`refused_parse`] reports.
pub fn limit_parses() {
    brush_parser::set_parse_budget(|tokens, chars| can_hold(parse_bytes(tokens, chars)));
}

/// The error for a parse stopped at the budget (see [`limit_parses`]), if `error` is one.
pub fn refused_parse(error: &brush_parser::ParseError) -> Option<error::Error> {
    match error {
        brush_parser::ParseError::Tokenizing {
            inner: brush_parser::TokenizerError::TooLarge { tokens, chars },
            ..
        } => Some(out_of_memory(parse_bytes(*tokens, *chars))),
        _ => None,
    }
}

/// Whether `bytes` more fit in the budget beside the `in_use` already held.
const fn fits(in_use: usize, bytes: usize) -> bool {
    match in_use.checked_add(bytes) {
        Some(total) => total <= MAX_SHELL_MEMORY_BYTES,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_fit_up_to_the_budget() {
        assert!(fits(0, MAX_SHELL_MEMORY_BYTES));
        assert!(fits(MAX_SHELL_MEMORY_BYTES - 10, 10));
        assert!(!fits(MAX_SHELL_MEMORY_BYTES - 10, 11));
        assert!(!fits(1, usize::MAX));
        assert!(reserve(MAX_SHELL_MEMORY_BYTES + 1).is_err());
    }

    #[test]
    fn a_refusal_ends_the_shell_as_bash_does() {
        let error = reserve(usize::MAX).unwrap_err();
        assert!(error.is_fatal());
        assert_eq!(
            error.to_string(),
            format!(
                "xmalloc: cannot allocate {} bytes: shell memory over 384 MiB is unsupported in \
                 bash-tool",
                usize::MAX
            )
        );
        assert_eq!(u8::from(crate::results::ExecutionExitCode::from(&error)), 2);
    }

    #[test]
    fn a_buffer_that_must_move_needs_room_for_twice_its_size() {
        assert!(reserve_growth(10, 100, 90).is_ok());
        assert!(
            reserve_growth(
                MAX_SHELL_MEMORY_BYTES / 2 + 1,
                MAX_SHELL_MEMORY_BYTES / 2 + 1,
                1
            )
            .is_err()
        );
        assert!(reserve_growth(0, 0, MAX_SHELL_MEMORY_BYTES + 1).is_err());
    }

    #[test]
    fn chunks_round_to_the_allocators_granule() {
        assert_eq!(allocation_bytes(0), 16);
        assert_eq!(allocation_bytes(12), 16);
        assert_eq!(allocation_bytes(13), 32);
        assert_eq!(allocation_bytes(1 << 20), (1 << 20) + 16);
    }

    #[test]
    fn a_parse_too_large_for_the_budget_is_refused() {
        limit_parses();
        let text = ":;".repeat(MAX_SHELL_MEMORY_BYTES / PARSE_TOKEN_BYTES);
        let error = brush_parser::Parser::new(
            std::io::BufReader::new(text.as_bytes()),
            &brush_parser::ParserOptions::default(),
        )
        .parse_program()
        .unwrap_err();
        let refused = refused_parse(&error).unwrap();
        assert!(refused.is_fatal());
        assert!(matches!(refused.kind(), error::ErrorKind::OutOfMemory(_)));
    }

    #[test]
    fn what_is_held_counts_against_the_budget() {
        let held = MAX_SHELL_MEMORY_BYTES / 4;
        allocated(held);
        let counted = in_use() >= held;
        released(held);
        assert!(counted);
        assert!(in_use() < held);
    }
}
