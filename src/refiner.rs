use crate::constants::*;
use crate::line_collector::NO_EOF_NEWLINE_MARKER_HOLDER;
use crate::token_collector::*;
use crate::tokenizer;
use diffus::{
    edit::{self, collection},
    Diffable,
};

/// Like format!(), but faster for our special case
fn format_simple_line(old_new: &str, plus_minus: char, contents: &str) -> String {
    let mut line = String::with_capacity(old_new.len() + 1 + contents.len() + NORMAL.len());
    line.push_str(old_new);
    line.push(plus_minus);
    line.push_str(contents);
    line.push_str(NORMAL);
    return line;
}

/// Format old and new lines in OLD and NEW colors.
///
/// No intra-line refinement.
#[must_use]
fn format_simple(old_text: &str, new_text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    for old_line in old_text.lines() {
        // Use a specialized line formatter since this code is in a hot path
        lines.push(format_simple_line(OLD, '-', old_line));
    }
    if (!old_text.is_empty()) && !old_text.ends_with('\n') {
        let no_eof_newline_marker_guard = NO_EOF_NEWLINE_MARKER_HOLDER.lock().unwrap();
        let no_eof_newline_marker = no_eof_newline_marker_guard.as_ref().unwrap();
        lines.push(format!(
            "{NO_EOF_NEWLINE_COLOR}{no_eof_newline_marker}{NORMAL}"
        ));
    }

    let announce_lost_newline = !new_text.is_empty() && !new_text.ends_with('\n');
    for (line_number, add_line) in new_text.lines().enumerate() {
        let new_line: String =
            if announce_lost_newline && line_number == new_text.lines().count() - 1 {
                // Add a red highlighted newline symbol at the end
                format!("{NEW}+{add_line}{OLD}{INVERSE_VIDEO}⏎{NORMAL}")
            } else {
                // Use a specialized line formatter since this code is in a hot path
                format_simple_line(NEW, '+', add_line)
            };
        lines.push(new_line);
    }
    if (!new_text.is_empty()) && !new_text.ends_with('\n') {
        let no_eof_newline_marker_guard = NO_EOF_NEWLINE_MARKER_HOLDER.lock().unwrap();
        let no_eof_newline_marker = no_eof_newline_marker_guard.as_ref().unwrap();
        lines.push(format!(
            "{NO_EOF_NEWLINE_COLOR}{no_eof_newline_marker}{NORMAL}"
        ));
    }

    return lines;
}

/// LCS is O(m * n) complexity. If it gets too complex, refining will take too
/// much time and memory, so we shouldn't.
///
/// Ref: https://github.com/walles/riff/issues/35
fn too_large_to_refine(old_text: &str, new_text: &str) -> bool {
    let complexity = (old_text.len() as u64) * (new_text.len() as u64);

    // Around this point refining starts taking near one second on Johan's
    // laptop. Numbers have been invented through experimentation.
    return complexity > 13_000u64 * 13_000u64;
}

/// Returns a vector of ANSI highlighted lines
#[must_use]
pub fn format(old_text: &str, new_text: &str) -> Vec<String> {
    if old_text.is_empty() || new_text.is_empty() {
        return format_simple(old_text, new_text);
    }

    if too_large_to_refine(old_text, new_text) {
        return format_simple(old_text, new_text);
    }

    let (old_tokens, new_tokens, old_highlights, new_unhighlighted) =
        to_highlighted_tokens(old_text, new_text);

    let highlighted_old_text;
    let highlighted_new_text;
    if old_highlights || new_unhighlighted || count_lines(&old_tokens) != count_lines(&new_tokens) {
        highlighted_old_text = render(&LINE_STYLE_OLD, old_tokens);
        highlighted_new_text = render(&LINE_STYLE_NEW, new_tokens);
    } else {
        highlighted_old_text = render(&LINE_STYLE_OLD_FAINT, old_tokens);
        highlighted_new_text = render(&LINE_STYLE_ADDS_ONLY, new_tokens);
    }

    return to_lines(&highlighted_old_text, &highlighted_new_text);
}

/// Returns two vectors for old and new sections. The first bool is true if
/// there were any highlights found in the old text. The second bool is true if
/// any highlights were removed for readability in the new text.
pub fn to_highlighted_tokens(
    old_text: &str,
    new_text: &str,
) -> (Vec<StyledToken>, Vec<StyledToken>, bool, bool) {
    // Find diffs between adds and removals
    let mut old_tokens = Vec::new();
    let mut new_tokens = Vec::new();

    // Tokenize adds and removes before diffing them
    let mut tokenized_old = tokenizer::tokenize(old_text);
    let mut tokenized_new = tokenizer::tokenize(new_text);

    // Help visualize what actually happens in "No newline at end of file" diffs
    if old_text.ends_with('\n') && !new_text.ends_with('\n') {
        tokenized_old.insert(tokenized_old.len() - 1, "⏎");
    } else if new_text.ends_with('\n') && !old_text.ends_with('\n') {
        tokenized_new.insert(tokenized_new.len() - 1, "⏎");
    }

    let diff = tokenized_old.diff(&tokenized_new);
    let mut old_highlights = false;
    match diff {
        edit::Edit::Copy(tokens) => {
            for &token in tokens {
                // FIXME: "Copy" means that old and new are the same, why was
                // format_split() called on this non-difference?
                //
                // Get here using "git show 686f3d7ae | cargo run" with git 2.35.1
                old_tokens.push(StyledToken::new(token.to_string(), Style::Plain));
                new_tokens.push(StyledToken::new(token.to_string(), Style::Plain));
            }
        }
        edit::Edit::Change(diff) => {
            diff.into_iter()
                .map(|edit| {
                    match edit {
                        collection::Edit::Copy(token) => {
                            old_tokens.push(StyledToken::new(token.to_string(), Style::Plain));
                            new_tokens.push(StyledToken::new(token.to_string(), Style::Plain));
                        }
                        collection::Edit::Insert(token) => {
                            new_tokens
                                .push(StyledToken::new(token.to_string(), Style::Highlighted));
                        }
                        collection::Edit::Remove(token) => {
                            old_tokens
                                .push(StyledToken::new(token.to_string(), Style::Highlighted));
                            old_highlights = true;
                        }
                        collection::Edit::Change(_) => {
                            unimplemented!("Edit/Change/Change not implemented, help!")
                        }
                    };
                })
                .for_each(drop);
        }
    }

    bridge_consecutive_highlighted_tokens(&mut old_tokens);
    unhighlight_noisy_rows(&mut old_tokens);

    bridge_consecutive_highlighted_tokens(&mut new_tokens);
    let new_unhighlighted = unhighlight_noisy_rows(&mut new_tokens);
    highlight_trailing_whitespace(&mut new_tokens);
    highlight_nonleading_tabs(&mut new_tokens);

    return (old_tokens, new_tokens, old_highlights, new_unhighlighted);
}

#[must_use]
fn to_lines(old: &str, new: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    for highlighted_old_line in old.lines() {
        lines.push(highlighted_old_line.to_string());
    }
    if (!old.is_empty()) && !old.ends_with('\n') {
        let no_eof_newline_marker_guard = NO_EOF_NEWLINE_MARKER_HOLDER.lock().unwrap();
        let no_eof_newline_marker = no_eof_newline_marker_guard.as_ref().unwrap();
        lines.push(format!(
            "{NO_EOF_NEWLINE_COLOR}{no_eof_newline_marker}{NORMAL}"
        ));
    }

    for highlighted_new_line in new.lines() {
        lines.push(highlighted_new_line.to_string());
    }
    if (!new.is_empty()) && !new.ends_with('\n') {
        let no_eof_newline_marker_guard = NO_EOF_NEWLINE_MARKER_HOLDER.lock().unwrap();
        let no_eof_newline_marker = no_eof_newline_marker_guard.as_ref().unwrap();
        lines.push(format!(
            "{NO_EOF_NEWLINE_COLOR}{no_eof_newline_marker}{NORMAL}"
        ));
    }

    return lines;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(test)]
    use pretty_assertions::assert_eq;

    #[test]
    fn test_simple_format_adds_and_removes() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(format_simple("", ""), empty);

        // Test adds-only
        assert_eq!(
            format_simple("", "a\n"),
            ["".to_string() + NEW + "+a" + NORMAL]
        );
        assert_eq!(
            format_simple("", "a\nb\n"),
            [
                "".to_string() + NEW + "+a" + NORMAL,
                "".to_string() + NEW + "+b" + NORMAL,
            ]
        );

        // Test removes-only
        assert_eq!(
            format_simple("a\n", ""),
            ["".to_string() + OLD + "-a" + NORMAL]
        );
        assert_eq!(
            format_simple("a\nb\n", ""),
            [
                "".to_string() + OLD + "-a" + NORMAL,
                "".to_string() + OLD + "-b" + NORMAL,
            ]
        );
    }

    #[test]
    fn test_quote_change() {
        // FIXME: Get this from somewhere else?
        const NOT_INVERSE_VIDEO: &str = "\x1b[27m";

        let result = format(
            "<unchanged text between quotes>\n",
            "[unchanged text between quotes]\n",
        );
        assert_eq!(
            result,
            [
                format!(
                    "{OLD}-{INVERSE_VIDEO}<{NOT_INVERSE_VIDEO}unchanged text between quotes{INVERSE_VIDEO}>{NORMAL}"
                ),
                format!(
                    "{NEW}+{INVERSE_VIDEO}[{NOT_INVERSE_VIDEO}unchanged text between quotes{INVERSE_VIDEO}]{NORMAL}"
                ),
            ]
        )
    }

    #[test]
    fn test_almost_empty_changes() {
        let result = format("x\n", "");
        assert_eq!(result, [format!("{OLD}-x{NORMAL}"),]);

        let result = format("", "x\n");
        assert_eq!(result, [format!("{NEW}+x{NORMAL}"),]);
    }
}
