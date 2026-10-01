use brush_parser::word;
use itertools::Itertools;

use crate::memory;

/// How many words a brace expansion gives, and about the bytes they take while they are expanded
/// (see [`crate::memory`]): a word's cost for each, and its text twice, as generated and as the
/// field it expands to.
pub(crate) fn expansion_size(pieces: &[word::BraceExpressionOrText]) -> (u128, usize) {
    let (count, text) = product_size(pieces);
    let bytes = count
        .saturating_mul(memory::WORD_BYTES as u128)
        .saturating_add(text.saturating_mul(2));
    (count, usize::try_from(bytes).unwrap_or(usize::MAX))
}

/// How many words `pieces` combine into, and the bytes of all their text.
fn product_size(pieces: &[word::BraceExpressionOrText]) -> (u128, u128) {
    let sizes: Vec<(u128, u128)> = pieces
        .iter()
        .map(|piece| match piece {
            word::BraceExpressionOrText::Text(text) => (1, text.len() as u128),
            word::BraceExpressionOrText::Expr(members) => members
                .iter()
                .map(member_size)
                .fold((0_u128, 0_u128), |acc, size| {
                    (acc.0.saturating_add(size.0), acc.1.saturating_add(size.1))
                }),
        })
        .collect();
    // Each word takes one alternative of every piece, so an alternative's text is in as many
    // words as the other pieces combine into.
    let count = sizes.iter().fold(1_u128, |count, (alternatives, _)| {
        count.saturating_mul(*alternatives)
    });
    let text = sizes.iter().fold(0_u128, |text, (alternatives, bytes)| {
        let others = count.checked_div(*alternatives).unwrap_or(0);
        text.saturating_add(bytes.saturating_mul(others))
    });
    (count, text)
}

/// How many words a member of a brace expression gives, and the bytes of all their text.
fn member_size(member: &word::BraceExpressionMember) -> (u128, u128) {
    match member {
        word::BraceExpressionMember::NumberSequence {
            start,
            end,
            increment,
            zero_padded_width,
        } => {
            let count = start.abs_diff(*end) / increment.unsigned_abs().max(1) + 1;
            // A member is at most as wide as the wider bound, its sign or its padding.
            let width = [start, end]
                .iter()
                .map(|bound| bound.to_string().len())
                .chain(zero_padded_width.iter().copied())
                .max()
                .unwrap_or(1);
            (u128::from(count), u128::from(count) * width as u128)
        }
        word::BraceExpressionMember::CharSequence {
            start,
            end,
            increment,
        } => {
            let span = u64::from(u32::from(*start).abs_diff(u32::from(*end)));
            let count = span / increment.unsigned_abs().max(1) + 1;
            // A character takes at most four bytes, or two for a quoted backslash.
            (u128::from(count), u128::from(count) * 4)
        }
        word::BraceExpressionMember::Child(elements) => product_size(elements),
    }
}

pub(crate) fn generate_and_combine_brace_expansions(
    pieces: Vec<brush_parser::word::BraceExpressionOrText>,
) -> impl IntoIterator<Item = String> {
    let expansions: Vec<Vec<String>> = pieces
        .into_iter()
        .map(|piece| expand_brace_expr_or_text(piece).collect())
        .collect();

    expansions
        .into_iter()
        .multi_cartesian_product()
        .map(|v| v.join(""))
}

fn expand_brace_expr_or_text(
    beot: word::BraceExpressionOrText,
) -> Box<dyn Iterator<Item = String>> {
    match beot {
        word::BraceExpressionOrText::Expr(members) => {
            // Chain all member iterators together
            Box::new(members.into_iter().flat_map(expand_brace_expr_member))
        }
        word::BraceExpressionOrText::Text(text) => Box::new(std::iter::once(text)),
    }
}

#[expect(clippy::cast_possible_truncation)]
fn expand_brace_expr_member(bem: word::BraceExpressionMember) -> Box<dyn Iterator<Item = String>> {
    match bem {
        word::BraceExpressionMember::NumberSequence {
            start,
            end,
            increment,
            zero_padded_width,
        } => {
            // Bash counts by the size of the increment toward `end`, and takes 0 as 1. The
            // steps are checked, so a sequence ends at either end of i64 instead of wrapping,
            // and stay in i64, since a usize is 32 bits on WASM.
            let step = increment.unsigned_abs().max(1);
            let up = start <= end;

            // A bound written with a leading zero asks for every member to be padded
            // out to the width of the longer bound, so `{01..10}` counts `01 02 ...`
            // and not `1 2 ...`. The sign takes one of those columns.
            let format = move |n: i64| match zero_padded_width {
                Some(width) => std::format!("{n:0width$}"),
                None => n.to_string(),
            };

            Box::new(
                std::iter::successors(Some(start), move |&n| {
                    let next = if up {
                        n.checked_add_unsigned(step)?
                    } else {
                        n.checked_sub_unsigned(step)?
                    };
                    let within = if up { next <= end } else { next >= end };
                    within.then_some(next)
                })
                .map(format),
            )
        }

        word::BraceExpressionMember::CharSequence {
            start,
            end,
            increment,
        } => {
            let mut increment = increment.unsigned_abs() as usize;
            if increment == 0 {
                increment = 1;
            }

            if start <= end {
                Box::new((start..=end).step_by(increment).map(sequence_char))
            } else {
                // Iterate from start down to end by decrementing.
                let increment = increment as u32;
                Box::new(
                    std::iter::successors(Some(start), move |&c| {
                        let next = char::from_u32((c as u32).checked_sub(increment)?)?;
                        (next >= end).then_some(next)
                    })
                    .map(sequence_char),
                )
            }
        }

        word::BraceExpressionMember::Child(elements) => {
            // Chain all element iterators together
            Box::new(generate_and_combine_brace_expansions(elements).into_iter())
        }
    }
}

/// A member of a letter sequence, as word text. A range from an upper-case to a lower-case letter
/// (`{Z..a}`) passes the punctuation between them; bash's quote removal takes the backslash
/// away, and keeps a backquote with nothing after it literal.
fn sequence_char(c: char) -> String {
    match c {
        '\\' => String::from("''"),
        '`' => String::from("\\`"),
        c => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(start: i64, end: i64, increment: i64) -> Vec<String> {
        let member = word::BraceExpressionMember::NumberSequence {
            start,
            end,
            increment,
            zero_padded_width: None,
        };
        // At most one more than bash gives, so a sequence that never ends fails instead of hangs.
        expand_brace_expr_member(member).take(3).collect()
    }

    #[test]
    fn number_sequences_stop_at_the_ends_of_i64() {
        assert_eq!(
            numbers(i64::MIN + 1, i64::MIN, 1),
            ["-9223372036854775807", "-9223372036854775808"]
        );
        assert_eq!(
            numbers(i64::MAX - 1, i64::MAX, 1),
            ["9223372036854775806", "9223372036854775807"]
        );
        assert_eq!(
            numbers(i64::MAX - 5, i64::MAX, 4),
            ["9223372036854775802", "9223372036854775806"]
        );
    }

    fn size_of_word(word: &str) -> (u128, u128) {
        let pieces = brush_parser::word::parse_brace_expansions(
            word,
            &brush_parser::ParserOptions::default(),
        )
        .unwrap()
        .unwrap();
        product_size(&pieces)
    }

    #[test]
    fn sizes_count_every_word_before_generating_any() {
        // 1..10: nine one-digit words and one of two digits, counted at the wider width.
        assert_eq!(size_of_word("{1..10}"), (10, 20));
        assert_eq!(size_of_word("x{a,bb}y"), (2, 7));
        assert_eq!(size_of_word("{a,b}{c,d}{e,f}"), (8, 24));
        assert_eq!(size_of_word("{1..100000000}").0, 100_000_000);
        assert_eq!(size_of_word("{a..e..2}").0, 3);
        // More words than a u64 counts.
        assert_eq!(
            size_of_word("{1..4000000000}{1..4000000000}{1..4000000000}").0,
            4_000_000_000_u128.pow(3)
        );
        let words = |word: &str| {
            let pieces = brush_parser::word::parse_brace_expansions(
                word,
                &brush_parser::ParserOptions::default(),
            )
            .unwrap()
            .unwrap();
            generate_and_combine_brace_expansions(pieces)
                .into_iter()
                .collect::<Vec<_>>()
        };
        for word in [
            "{1..10}",
            "x{a,bb}y",
            "{a,b}{c,{d..f}}",
            "{-5..5..3}",
            "{01..12}",
        ] {
            let generated = words(word);
            let (count, text) = size_of_word(word);
            assert_eq!(count, generated.len() as u128, "{word}");
            assert!(
                text >= generated.iter().map(String::len).sum::<usize>() as u128,
                "{word}"
            );
        }
    }

    #[test]
    fn number_sequence_increments_are_not_truncated() {
        // 2^32 would truncate to 0 in a 32-bit usize, and then count by 1.
        assert_eq!(numbers(1, 3, 1 << 32), ["1"]);
        assert_eq!(numbers(3, 1, 1 << 32), ["3"]);
        assert_eq!(numbers(1, 3, 0), ["1", "2", "3"]);
    }
}
