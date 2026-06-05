//! Comment and blank-line trivia attachment.
//!
//! This is the formatter's hardest correctness problem and exactly where
//! `gdformat` breaks (its `CommentPersistenceViolation`): comments and blank
//! lines are not part of the "meaningful" syntax tree, so a naive lowering drops
//! or relocates them. We make trivia a *first-class* concern.
//!
//! In tree-sitter-gdscript, comments are real named `(comment)` nodes
//! interleaved with statements in a block's child list (they are **not** hidden
//! `extra` nodes). Blank lines are not nodes at all — they survive only as row
//! gaps between consecutive children. This module turns a flat, row-annotated
//! child sequence into a list of [`Item`]s, each carrying the statement plus the
//! trivia that belongs to it:
//!
//! - **leading** comments — own-line comments above a statement (incl. `##` doc
//!   comments and the top-of-file banner);
//! - **trailing** comment — a same-line comment after a statement (`x = 1  # c`);
//! - **blank_before** — whether a run of one or more blank lines preceded the
//!   item (runs collapse to a single blank: presence is preserved, not count).
//!
//! The attachment logic is generic over the statement payload `T` so it can be
//! unit-tested without tree-sitter (`T = &str`) and instantiated over
//! `tree_sitter::Node` by the lowering pass. This mirrors how Prettier separates
//! comment *attachment* from comment *printing*.

/// A GDScript comment, classified by marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub kind: CommentKind,
    /// The full comment text including its leading marker, trailing whitespace
    /// trimmed. E.g. `"# hello"` or `"## a doc comment"`.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// A regular `#` comment.
    Regular,
    /// A `##` documentation comment (Godot surfaces these in the editor docs).
    Doc,
}

impl Comment {
    /// Parse a raw comment token (the source text of a `(comment)` node).
    pub fn parse(raw: &str) -> Comment {
        let text = raw.trim_end().to_string();
        // `##` is a doc comment; `#!` and `#` are regular. Check `##` before `#`.
        let kind = if text.starts_with("##") {
            CommentKind::Doc
        } else {
            CommentKind::Regular
        };
        Comment { kind, text }
    }
}

/// One element of a raw block child sequence: either a comment or a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element<T> {
    Comment(Comment),
    Stmt(T),
}

/// A row-annotated block child handed to [`attach`]. Rows are 0-based source
/// lines; `start_row`/`end_row` delimit the element's vertical extent so blank
/// runs between elements can be recovered.
pub struct RawElement<T> {
    pub element: Element<T>,
    pub start_row: usize,
    pub end_row: usize,
}

/// A statement with its attached trivia, ready for lowering.
///
/// `stmt` is `None` for a *dangling* comment group — comments with no following
/// statement (e.g. a comment as the last line of a block). Those must still be
/// emitted so nothing is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item<T> {
    /// Own-line comments that precede the statement, in source order.
    pub leading: Vec<Comment>,
    /// The statement payload, or `None` for a trailing dangling comment group.
    pub stmt: Option<T>,
    /// A same-line comment after the statement.
    pub trailing: Option<Comment>,
    /// Whether at least one blank line preceded this item.
    pub blank_before: bool,
}

/// Attach trivia to statements in a block child sequence.
///
/// Classification rules:
/// - A comment whose `start_row` equals the immediately preceding statement's
///   `end_row`, with no leading comments already buffered, is that statement's
///   **trailing** (inline) comment.
/// - Any other comment is **leading**: it is buffered and attached to the next
///   statement (or becomes a dangling group if none follows).
/// - `blank_before` is set when an item's first row is more than one line below
///   the previous item's last row (a blank run, collapsed to presence).
pub fn attach<T>(elements: impl IntoIterator<Item = RawElement<T>>) -> Vec<Item<T>> {
    // Build items, tracking each item's vertical extent for blank detection.
    struct Pending<T> {
        item: Item<T>,
        first_row: usize,
        last_row: usize,
    }
    let mut pending: Vec<Pending<T>> = Vec::new();
    let mut leading: Vec<Comment> = Vec::new();
    let mut leading_first_row: Option<usize> = None;
    let mut last_stmt_end_row: Option<usize> = None;

    for raw in elements {
        match raw.element {
            Element::Comment(c) => {
                let is_inline = leading.is_empty()
                    && last_stmt_end_row == Some(raw.start_row)
                    && pending
                        .last()
                        .map(|p| p.item.stmt.is_some() && p.item.trailing.is_none())
                        .unwrap_or(false);
                if is_inline {
                    let p = pending.last_mut().unwrap();
                    p.item.trailing = Some(c);
                    p.last_row = raw.end_row;
                    // An inline comment does not start a new statement; clear the
                    // anchor so a following own-line comment is treated as leading.
                    last_stmt_end_row = None;
                } else {
                    if leading.is_empty() {
                        leading_first_row = Some(raw.start_row);
                    }
                    leading.push(c);
                }
            }
            Element::Stmt(payload) => {
                let first_row = leading_first_row.take().unwrap_or(raw.start_row);
                pending.push(Pending {
                    item: Item {
                        leading: std::mem::take(&mut leading),
                        stmt: Some(payload),
                        trailing: None,
                        blank_before: false,
                    },
                    first_row,
                    last_row: raw.end_row,
                });
                last_stmt_end_row = Some(raw.end_row);
            }
        }
    }
    // Flush any remaining leading comments as a dangling group.
    if !leading.is_empty() {
        let first_row = leading_first_row.unwrap_or(0);
        let last_row = first_row;
        pending.push(Pending {
            item: Item {
                leading,
                stmt: None,
                trailing: None,
                blank_before: false,
            },
            first_row,
            last_row,
        });
    }

    // Second pass: compute blank_before from row gaps.
    let mut prev_last_row: Option<usize> = None;
    let mut out = Vec::with_capacity(pending.len());
    for mut p in pending {
        if let Some(prev) = prev_last_row {
            if p.first_row > prev + 1 {
                p.item.blank_before = true;
            }
        }
        prev_last_row = Some(p.last_row);
        out.push(p.item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(row: usize, text: &str) -> RawElement<&'static str> {
        RawElement {
            element: Element::Comment(Comment::parse(text)),
            start_row: row,
            end_row: row,
        }
    }
    fn stmt(row: usize, name: &'static str) -> RawElement<&'static str> {
        RawElement {
            element: Element::Stmt(name),
            start_row: row,
            end_row: row,
        }
    }

    #[test]
    fn parse_distinguishes_doc_from_regular() {
        assert_eq!(Comment::parse("# hi").kind, CommentKind::Regular);
        assert_eq!(Comment::parse("## hi").kind, CommentKind::Doc);
        assert_eq!(Comment::parse("### hi").kind, CommentKind::Doc);
        assert_eq!(Comment::parse("#! shebang-ish").kind, CommentKind::Regular);
        assert_eq!(Comment::parse("# trailing ws   ").text, "# trailing ws");
    }

    #[test]
    fn leading_comment_attaches_to_next_statement() {
        let items = attach([comment(0, "# lead"), stmt(1, "a")]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].leading.len(), 1);
        assert_eq!(items[0].leading[0].text, "# lead");
        assert_eq!(items[0].stmt, Some("a"));
        assert!(items[0].trailing.is_none());
    }

    #[test]
    fn inline_comment_attaches_to_previous_statement() {
        // `a  # inline` — both on row 0.
        let items = attach([stmt(0, "a"), comment(0, "# inline")]);
        assert_eq!(items.len(), 1);
        assert!(items[0].leading.is_empty());
        assert_eq!(items[0].trailing.as_ref().unwrap().text, "# inline");
    }

    #[test]
    fn comment_on_own_line_after_stmt_is_leading_not_inline() {
        // Comment on row 1, statement ended on row 0 → leading for next stmt.
        let items = attach([stmt(0, "a"), comment(1, "# lead"), stmt(2, "b")]);
        assert_eq!(items.len(), 2);
        assert!(items[0].trailing.is_none());
        assert_eq!(items[1].leading[0].text, "# lead");
    }

    #[test]
    fn blank_run_sets_presence_flag_once() {
        // a (row0), then 2 blank lines, then b (row3).
        let items = attach([stmt(0, "a"), stmt(3, "b")]);
        assert!(!items[0].blank_before);
        assert!(items[1].blank_before, "blank run should be preserved");
    }

    #[test]
    fn adjacent_statements_have_no_blank() {
        let items = attach([stmt(0, "a"), stmt(1, "b")]);
        assert!(!items[1].blank_before);
    }

    #[test]
    fn blank_before_leading_comment_counts_for_the_item() {
        // a(row0), blank, # lead(row2), b(row3). The blank precedes the leading
        // comment, so the *item* (comment+b) is blank_before.
        let items = attach([stmt(0, "a"), comment(2, "# lead"), stmt(3, "b")]);
        assert_eq!(items.len(), 2);
        assert!(items[1].blank_before);
        assert_eq!(items[1].leading[0].text, "# lead");
    }

    #[test]
    fn dangling_comment_at_block_end_is_kept() {
        let items = attach([stmt(0, "a"), comment(1, "# trailing block")]);
        assert_eq!(items.len(), 2);
        assert!(items[1].stmt.is_none());
        assert_eq!(items[1].leading[0].text, "# trailing block");
    }

    #[test]
    fn doc_comment_block_stays_leading() {
        let items = attach([
            comment(0, "## Doc line 1"),
            comment(1, "## Doc line 2"),
            stmt(2, "func"),
        ]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].leading.len(), 2);
        assert!(items[0].leading.iter().all(|c| c.kind == CommentKind::Doc));
    }

    #[test]
    fn inline_then_ownline_comment() {
        // a  # inline(row0), then # lead(row1), b(row2).
        let items = attach([
            stmt(0, "a"),
            comment(0, "# inline"),
            comment(1, "# lead"),
            stmt(2, "b"),
        ]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].trailing.as_ref().unwrap().text, "# inline");
        assert_eq!(items[1].leading[0].text, "# lead");
    }
}
