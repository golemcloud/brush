//! Implements parsing for shell glob and extglob patterns.

use crate::error;

/// Represents the kind of an extended glob.
pub enum ExtendedGlobKind {
    /// The `+` extended glob; matches one or more occurrences of the inner pattern.
    Plus,
    /// The `@` extended glob; allows matching an alternation of inner patterns.
    At,
    /// The `!` extended glob; matches the negation of the inner pattern.
    Exclamation,
    /// The `?` extended glob; matches zero or one occurrence of the inner pattern.
    Question,
    /// The `*` extended glob; matches zero or more occurrences of the inner pattern.
    Star,
}

/// Converts a shell pattern to a regular expression string.
///
/// # Arguments
///
/// * `pattern` - The shell pattern to convert.
/// * `enable_extended_globbing` - Whether to enable extended globbing (extglob).
pub fn pattern_to_regex_str(
    pattern: &str,
    enable_extended_globbing: bool,
) -> Result<String, error::WordParseError> {
    let limits = Limits::default();
    let regex_str =
        pattern_to_regex_translator::pattern(pattern, enable_extended_globbing, &limits);
    if limits.exceeded.get() {
        return Err(error::WordParseError::NestedTooDeeply);
    }
    regex_str.map_err(|e| error::WordParseError::Pattern(e.into()))
}

/// How deeply extended globs may nest. Translating one recurses once per level, and each level
/// nests the regular expression one or two groups deeper, which the regex engine bounds.
const MAX_EXTGLOB_NESTING: usize = 24;

/// The most text the negated extended globs of one pattern may translate their bodies to,
/// together. `!(p)` holds `p`'s regular expression twice, so nesting negations doubles the
/// translation at each level.
const MAX_NEGATED_BODY_BYTES: usize = 512 * 1024;

/// What translating one pattern has used of its limits.
#[derive(Default)]
struct Limits {
    /// Whether the pattern nests deeper, or negates more text, than a pattern may.
    exceeded: std::cell::Cell<bool>,
    /// The text the pattern's negated extended globs have translated their bodies to so far.
    negated_bytes: std::cell::Cell<usize>,
}

/// Formats a range between two bracket expression members, returning `None` if the
/// range is reversed (and therefore matches nothing).
fn bracket_range(from: (String, char), to: (String, char)) -> Option<String> {
    let (from_str, from_c) = from;
    let (to_str, to_c) = to;

    if from_c <= to_c {
        Some(std::format!("{from_str}-{to_str}"))
    } else {
        None
    }
}

peg::parser! {
    grammar pattern_to_regex_translator<'a>(enable_extended_globbing: bool, limits: &'a Limits) for str {
        pub(crate) rule pattern() -> String =
            pieces:(pattern_piece(0)*) {
                pieces.join("")
            }

        rule pattern_piece(depth: usize) -> String =
            escape_sequence() /
            bracket_expression() /
            extglob_enabled() s:extended_glob_pattern(depth) { s } /
            wildcard() /
            [c if regex_char_needs_escaping(c)] {
                let mut s = '\\'.to_string();
                s.push(c);
                s
            } /
            [c] { c.to_string() }

        rule escape_sequence() -> String =
            sequence:$(['\\'] [c if regex_char_needs_escaping(c)]) { sequence.to_owned() } /
            ['\\'] [c] { c.to_string() }

        rule bracket_expression() -> String =
            "[" invert:(invert_char()?) leading:(leading_bracket_member()?) members:bracket_member()* "]" {
                let (classes, members): (Vec<_>, Vec<_>) = leading
                    .into_iter()
                    .flatten()
                    .map(|member| (member, false))
                    .chain(members.into_iter().flatten())
                    .partition(|(_, class)| *class);
                let mut members: Vec<String> =
                    members.into_iter().map(|(member, _)| member).collect();

                // A character class matches its own characters even where case does not
                // otherwise count (`nocaseglob`, `nocasematch`), as in bash.
                if !classes.is_empty() {
                    let classes: String = classes.into_iter().map(|(class, _)| class).collect();
                    let classes = std::format!("(?-i:[{classes}])");
                    let any = if members.is_empty() {
                        classes
                    } else {
                        std::format!("(?:[{}]|{classes})", members.join(""))
                    };
                    return if invert.is_some() {
                        std::format!("(?:(?!{any})(?s:.))")
                    } else {
                        any
                    };
                }

                // If we completed the parse but ended up with no valid members
                // of the bracket expression, then return a regex that matches nothing.
                // (Or in the inverted case, matches everything.)
                if members.is_empty() {
                    if invert.is_some() {
                        String::from(".")
                    } else {
                        String::from("(?!)")
                    }
                } else {
                    if invert.is_some() {
                        members.insert(0, String::from("^"));
                    }

                    std::format!("[{}]", members.join(""))
                }
            }

        // A ']' first in the expression (after any '!' or '^') is a member, not the end.
        // It may also be the low end of a range, so it goes through the same range
        // validation as any other member.
        rule leading_bracket_member() -> Option<String> =
            from:close_bracket_member() "-" to:single_char_bracket_member() {
                bracket_range(from, to)
            } /
            from:close_bracket_member() { Some(from.0) }

        rule close_bracket_member() -> (String, char) =
            "]" { (String::from(r"\]"), ']') }

        rule invert_char() -> bool =
            ['!' | '^'] { true }

        // A member, and whether it is a character class.
        rule bracket_member() -> Option<(String, bool)> =
            e:char_class_expression() { Some((e, true)) } /
            r:char_range() { r.map(|r| (r, false)) } /
            m:single_char_bracket_member() {
                let (char_str, _) = m;
                Some((char_str, false))
            }

        rule char_class_expression() -> String =
            e:$("[:" char_class() ":]") { e.to_owned() }

        rule char_class() =
            "alnum" / "alpha" / "blank" / "cntrl" / "digit" / "graph" / "lower" / "print" / "punct" / "space" / "upper"/ "xdigit"

        rule char_range() -> Option<String> =
            from:single_char_bracket_member() "-" to:single_char_bracket_member() {
                bracket_range(from, to)
            }

        rule single_char_bracket_member() -> (String, char) =
            // Preserve escaped characters as-is.
            ['\\'] [c] { (std::format!("\\{c}"), c) } /
            // Escape the characters that are special inside a regex character class;
            // left bare, a '^' first in the class would negate it and a '-' would be
            // read as a range separator.
            [c if matches!(c, '[' | '^' | '-')] { (std::format!("\\{c}"), c) } /
            // Any other character except closing bracket gets added as-is.
            [c if c != ']'] { (c.to_string(), c) }

        rule wildcard() -> String =
            "?" { String::from(".") } /
            "*" { String::from(".*") }

        rule extglob_enabled() -> () =
            &[_] {? if enable_extended_globbing { Ok(()) } else { Err("extglob disabled") } }

        pub(crate) rule extended_glob_pattern(depth: usize) -> String =
            kind:extended_glob_prefix() "(" nested(depth) body_nullable:&(n:(nullable_pieces(depth + 1) ** "|") { n.into_iter().any(|n| n) }) branches:extended_glob_body(depth + 1) ")" {?
                let mut s = String::new();

                // fancy_regex uses ?! to indicate a negative lookahead.
                if matches!(kind, ExtendedGlobKind::Exclamation) {
                    if !branches.is_empty() {
                        let body = branches.join("|");
                        let negated_bytes = limits.negated_bytes.get() + body.len();
                        if negated_bytes > MAX_NEGATED_BODY_BYTES {
                            limits.exceeded.set(true);
                            return Err("a smaller negated pattern");
                        }
                        limits.negated_bytes.set(negated_bytes);
                        s.push_str("(?:(?!");
                        s.push_str(&body);
                        s.push_str(").*|(?>");
                        s.push_str(&body);
                        s.push_str(").+?");
                        // The empty string, unless the inner pattern matches it (`!(*)`).
                        if !body_nullable {
                            s.push('|');
                        }
                        s.push(')');
                    } else {
                        s.push_str("(?:.+)");
                    }
                } else {
                    s.push('(');
                    s.push_str(&branches.join("|"));
                    s.push(')');

                    match kind {
                        ExtendedGlobKind::Plus => s.push('+'),
                        ExtendedGlobKind::Question => s.push('?'),
                        ExtendedGlobKind::Star => s.push('*'),
                        ExtendedGlobKind::At | ExtendedGlobKind::Exclamation => (),
                    }
                }

                Ok(s)
            }

        // Past the nesting an extended glob may have, fails and marks the pattern as too deep.
        rule nested(depth: usize) =
            "" {?
                if depth < MAX_EXTGLOB_NESTING && !limits.exceeded.get() {
                    Ok(())
                } else {
                    limits.exceeded.set(true);
                    Err("shallower nesting")
                }
            }

        rule extended_glob_prefix() -> ExtendedGlobKind =
            "+" { ExtendedGlobKind::Plus } /
            "@" { ExtendedGlobKind::At } /
            "!" { ExtendedGlobKind::Exclamation } /
            "?" { ExtendedGlobKind::Question } /
            "*" { ExtendedGlobKind::Star }

        pub(crate) rule extended_glob_body(depth: usize) -> Vec<String> =
            // Cover case with *no* branches.
            &[')'] { vec![] } /
            // Otherwise, look for branches separated by '|'.
            extended_glob_branch(depth) ** "|"

        rule extended_glob_branch(depth: usize) -> String =
            // Cover case of empty branch.
            &['|' | ')'] { String::new() } /
            pieces:(!['|' | ')'] piece:pattern_piece(depth) { piece })+ {
                pieces.join("")
            }

        // Whether the pieces of a pattern, up to the `|` or `)` that ends an extglob branch, can
        // match the empty string.
        rule nullable_pieces(depth: usize) -> bool =
            n:(!['|' | ')'] n:nullable_piece(depth) { n })* { n.into_iter().all(|n| n) }

        rule nullable_piece(depth: usize) -> bool =
            escape_sequence() { false } /
            bracket_expression() { false } /
            extglob_enabled() n:nullable_extended_glob(depth) { n } /
            "*" { true } /
            [_] { false }

        rule nullable_extended_glob(depth: usize) -> bool =
            kind:extended_glob_prefix() "(" nested(depth) n:(nullable_pieces(depth + 1) ** "|") ")" {
                let any = n.into_iter().any(|n| n);
                match kind {
                    ExtendedGlobKind::Star | ExtendedGlobKind::Question => true,
                    ExtendedGlobKind::At | ExtendedGlobKind::Plus => any,
                    ExtendedGlobKind::Exclamation => !any,
                }
            }

        // A glob metacharacter construct: wildcard, bracket expression, or extglob.
        rule glob_piece() =
            bracket_expression() /
            extglob_enabled() extended_glob_pattern(0) /
            wildcard()

        // A non-glob piece: an escape sequence or any character not starting a glob.
        rule non_glob_piece() =
            escape_sequence() /
            !glob_piece() [_]

        // Succeeds (returning true) if the pattern contains at least one glob
        // metacharacter. The same bracket_expression, wildcard, and
        // extended_glob_pattern rules used for regex conversion are reused here
        // via negative lookaheads, keeping a single source of truth.
        pub(crate) rule has_glob_metacharacters() -> bool =
            non_glob_piece()* glob_piece() [_]* { true }
    }
}

/// Returns whether a pattern string contains any glob metacharacters.
///
/// Uses the same PEG grammar rules that `pattern_to_regex_str` uses, keeping
/// a single source of truth for what constitutes a glob metacharacter.
///
/// # Arguments
///
/// * `pattern` - The shell pattern to check.
/// * `enable_extended_globbing` - Whether to enable extended globbing (extglob).
pub fn pattern_has_glob_metacharacters(pattern: &str, enable_extended_globbing: bool) -> bool {
    let limits = Limits::default();
    let found = pattern_to_regex_translator::has_glob_metacharacters(
        pattern,
        enable_extended_globbing,
        &limits,
    )
    .unwrap_or(false);
    // A pattern nested too deeply to tell is one, so that translating it reports why it fails.
    found || limits.exceeded.get()
}

/// Returns whether or not a given character needs to be escaped in a regular expression.
///
/// # Arguments
///
/// * `c` - The character to check.
pub const fn regex_char_needs_escaping(c: char) -> bool {
    matches!(
        c,
        '[' | ']' | '(' | ')' | '{' | '}' | '*' | '?' | '.' | '+' | '^' | '$' | '|' | '\\' | '-'
    )
}

#[cfg(test)]
#[expect(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn test_negated_extglob_matches_empty_only_when_inner_cannot() -> Result<()> {
        // `!(*)` matches nothing, not even the empty string; `!(a)` matches it.
        assert_eq!(
            pattern_to_regex_str("!(*)", true)?,
            "(?:(?!.*).*|(?>.*).+?)"
        );
        assert_eq!(pattern_to_regex_str("!(a)", true)?, "(?:(?!a).*|(?>a).+?|)");
        assert_eq!(
            pattern_to_regex_str("!(x|*(y))", true)?,
            "(?:(?!x|(y)*).*|(?>x|(y)*).+?)"
        );
        Ok(())
    }

    #[test]
    fn test_bracket_exprs() -> Result<()> {
        assert_eq!(pattern_to_regex_str("[a-z]", true)?, "[a-z]");
        assert_eq!(pattern_to_regex_str("[z-a]", true)?, "(?!)");
        assert_eq!(pattern_to_regex_str("[+-/]", true)?, "[+-/]");
        assert_eq!(pattern_to_regex_str(r"[\*-/]", true)?, r"[\*-/]");
        assert_eq!(pattern_to_regex_str("[abc]", true)?, "[abc]");
        assert_eq!(pattern_to_regex_str(r"[\(]", true)?, r"[\(]");
        assert_eq!(pattern_to_regex_str(r"[(]", true)?, "[(]");
        assert_eq!(
            pattern_to_regex_str("[[:digit:]]", true)?,
            "(?-i:[[:digit:]])"
        );
        assert_eq!(
            pattern_to_regex_str("[![:upper:]x-z]", true)?,
            "(?:(?!(?:[x-z]|(?-i:[[:upper:]])))(?s:.))"
        );
        assert_eq!(pattern_to_regex_str(r"[-(),!]*", true)?, r"[\-(),!].*");
        assert_eq!(
            pattern_to_regex_str(r"[-\(\),\!]*", true)?,
            r"[\-\(\),\!].*"
        );
        assert_eq!(pattern_to_regex_str(r"[a\-b]", true)?, r"[a\-b]");
        assert_eq!(pattern_to_regex_str(r"[a\-\*]", true)?, r"[a\-\*]");

        // A ']' in the first member position (after any '!' or '^') is a member of the
        // bracket expression, not its terminator.
        assert_eq!(pattern_to_regex_str("[]]", true)?, r"[\]]");
        assert_eq!(pattern_to_regex_str("[][]", true)?, r"[\]\[]");
        assert_eq!(pattern_to_regex_str("[]abc[]", true)?, r"[\]abc\[]");
        assert_eq!(pattern_to_regex_str("[]]]", true)?, r"[\]]\]");
        assert_eq!(pattern_to_regex_str("[!]]", true)?, r"[^\]]");
        assert_eq!(pattern_to_regex_str("[^][]", true)?, r"[^\]\[]");

        // A range starting at that ']' is validated like any other range.
        assert_eq!(pattern_to_regex_str("[]-a]", true)?, r"[\]-a]");
        assert_eq!(pattern_to_regex_str("[]-a[]", true)?, r"[\]-a\[]");
        assert_eq!(pattern_to_regex_str("[]-A]", true)?, "(?!)");
        assert_eq!(pattern_to_regex_str("[!]-A]", true)?, ".");

        // '^' and '-' are escaped as members, so a '^' left first by a dropped
        // range can't negate the class and a '-' endpoint can't be read as a
        // range separator.
        assert_eq!(pattern_to_regex_str("[9-0^a]", true)?, r"[\^a]");
        assert_eq!(pattern_to_regex_str("[z-a^]", true)?, r"[\^]");
        assert_eq!(pattern_to_regex_str("[+--]", true)?, r"[+-\-]");
        assert_eq!(pattern_to_regex_str("[--/]", true)?, r"[\--/]");

        // An unterminated bracket expression is still just literal text.
        assert_eq!(pattern_to_regex_str("[]", true)?, r"\[\]");
        assert_eq!(pattern_to_regex_str("[!]", true)?, r"\[!\]");

        Ok(())
    }

    #[test]
    fn test_extended_glob() -> Result<()> {
        let limits = Limits::default();
        let pattern =
            |text| pattern_to_regex_translator::extended_glob_pattern(text, true, &limits, 0);
        let body = |text| pattern_to_regex_translator::extended_glob_body(text, true, &limits, 0);
        assert_eq!(pattern("@(a|b)")?, "(a|b)");
        assert_eq!(pattern("@(|a)")?, "(|a)");
        assert_eq!(pattern("@(|)")?, "(|)");
        assert_eq!(body("ab|ac")?, vec!["ab", "ac"]);
        assert_eq!(pattern("*(ab|ac)")?, "(ab|ac)*");
        assert_eq!(body("")?, Vec::<String>::new());
        assert!(!limits.exceeded.get());

        Ok(())
    }

    #[test]
    fn extended_globs_nest_up_to_the_limit() {
        let nested = |kind: &str, depth: usize| {
            std::format!(
                "{}a{}",
                std::format!("{kind}(").repeat(depth),
                ")".repeat(depth)
            )
        };
        for kind in ["@", "!", "+", "?", "*"] {
            // Nested negations reach the limit on their translation first (see below).
            let deepest = if kind == "!" { 12 } else { MAX_EXTGLOB_NESTING };
            let pattern = nested(kind, deepest);
            assert!(pattern_to_regex_str(&pattern, true).is_ok(), "{pattern}");
            // Deeper fails, however deep, without recursing further; so does telling whether
            // it is a pattern at all.
            for depth in [MAX_EXTGLOB_NESTING + 1, 10_000] {
                let pattern = nested(kind, depth);
                assert!(matches!(
                    pattern_to_regex_str(&pattern, true),
                    Err(error::WordParseError::NestedTooDeeply)
                ));
                assert!(pattern_has_glob_metacharacters(&pattern, true));
            }
            // Without extglob, the text is not a pattern to nest.
            assert!(pattern_to_regex_str(&nested(kind, 10_000), false).is_ok());
        }
    }

    #[test]
    fn nested_negations_stop_before_their_translation_explodes() {
        // Each `!( )` holds its body twice, so twenty levels would take 2^20 times the text.
        let pattern = std::format!("{}abcdefgh{}", "!(".repeat(20), ")".repeat(20));
        assert!(matches!(
            pattern_to_regex_str(&pattern, true),
            Err(error::WordParseError::NestedTooDeeply)
        ));
        let pattern = std::format!("{}abcdefgh{}", "!(".repeat(8), ")".repeat(8));
        assert!(pattern_to_regex_str(&pattern, true).is_ok());
        // Nor does a pattern of many negations, each within the limit, add up past it.
        let negation = std::format!("{}abcdefgh{}", "!(".repeat(10), ")".repeat(10));
        assert!(pattern_to_regex_str(&negation, true).is_ok());
        assert!(matches!(
            pattern_to_regex_str(&negation.repeat(1000), true),
            Err(error::WordParseError::NestedTooDeeply)
        ));
    }

    #[test]
    fn test_has_glob_metacharacters() {
        // Basic metacharacters.
        assert!(pattern_has_glob_metacharacters("*", false));
        assert!(pattern_has_glob_metacharacters("?", false));
        assert!(pattern_has_glob_metacharacters("a*b", false));
        assert!(pattern_has_glob_metacharacters("a?b", false));

        // Valid bracket expressions.
        assert!(pattern_has_glob_metacharacters("[abc]", false));
        assert!(pattern_has_glob_metacharacters("[a-z]", false));
        assert!(pattern_has_glob_metacharacters("[!a]", false));

        // A leading ']' is a member, so these are bracket expressions too.
        assert!(pattern_has_glob_metacharacters("[]]", false));
        assert!(pattern_has_glob_metacharacters("[][]", false));
        assert!(pattern_has_glob_metacharacters("[!]]", false));

        // Lone `]` is NOT a glob metacharacter.
        assert!(!pattern_has_glob_metacharacters("]", false));
        assert!(!pattern_has_glob_metacharacters("foo]", false));
        assert!(!pattern_has_glob_metacharacters("a]b", false));

        // Lone `[` without matching `]` is NOT a glob metacharacter.
        assert!(!pattern_has_glob_metacharacters("[", false));
        assert!(!pattern_has_glob_metacharacters("[]", false));
        assert!(!pattern_has_glob_metacharacters("[!]", false));
        assert!(!pattern_has_glob_metacharacters("[abc", false));
        assert!(!pattern_has_glob_metacharacters("a[b", false));

        // Plain text — no glob chars.
        assert!(!pattern_has_glob_metacharacters("hello", false));
        assert!(!pattern_has_glob_metacharacters("", false));

        // Backslash-escaped metacharacters are not globs.
        assert!(!pattern_has_glob_metacharacters(r"\*", false));
        assert!(!pattern_has_glob_metacharacters(r"\?", false));
        assert!(!pattern_has_glob_metacharacters(r"\[abc]", false));

        // Extglob patterns — not detected without extended globbing.
        assert!(!pattern_has_glob_metacharacters("@(a)", false));
        assert!(!pattern_has_glob_metacharacters("!(a)", false));
        assert!(!pattern_has_glob_metacharacters("+(a)", false));

        // Extglob patterns — detected with extended globbing.
        assert!(pattern_has_glob_metacharacters("@(a)", true));
        assert!(pattern_has_glob_metacharacters("!(a)", true));
        assert!(pattern_has_glob_metacharacters("+(a)", true));

        // *( and ?( are already caught by * and ? checks.
        assert!(pattern_has_glob_metacharacters("*(a)", false));
        assert!(pattern_has_glob_metacharacters("?(a)", false));
    }
}
