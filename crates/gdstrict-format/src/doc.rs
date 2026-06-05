//! A Wadler/Prettier-style document IR and width-aware renderer.
//!
//! This is the core layout engine for the "black for Godot" formatter: build a
//! `Doc` tree, then render it at a target line width. `Group`s render flat if they
//! fit on the remaining line, otherwise they break — turning `Line`/`SoftLine` into
//! newlines and emitting `IfBreak` content (e.g. a magic trailing comma).
//!
//! Algorithm is the standard Prettier print loop (a work stack of
//! `(indent, mode, doc)` plus a `fits` lookahead). See:
//! Wadler, "A prettier printer" (2003); Prettier's `doc-printer`.

/// Visual width of one indent level, in columns. GDScript is tab-indented (one
/// TAB per level — see [`indent_str`]), but the layout engine still needs a
/// *column cost* per level to decide whether a group fits within the line width;
/// a tab is budgeted as this many columns. Because `ind` only ever advances by
/// `INDENT`, `ind / INDENT` recovers the nesting level for tab rendering.
pub const INDENT: usize = 4;

/// The indentation string for a given column depth `ind`. GDScript uses tabs, so
/// this is one TAB per nesting level (`ind / INDENT`), not `ind` spaces.
fn indent_str(ind: usize) -> String {
    "\t".repeat(ind / INDENT)
}

#[derive(Debug, Clone)]
pub enum Doc {
    /// Literal text. Must not contain newlines.
    Text(String),
    /// Literal text emitted exactly as-is, newlines and all, with NO
    /// re-indentation. This is a *content* leaf (not a layout primitive): it
    /// exists so lossless tokens whose bytes span multiple source lines —
    /// triple-quoted strings being the motivating case — survive formatting
    /// byte-for-byte. The layout engine never inserts or rewrites a `Verbatim`;
    /// it only places it. All wrapping is still expressed with
    /// `Group`/`Indent`/`Line`/`SoftLine`/`IfBreak`.
    Verbatim(String),
    /// Concatenation.
    Concat(Vec<Doc>),
    /// A break opportunity: a space when flat, a newline when broken.
    Line,
    /// A soft break: nothing when flat, a newline when broken.
    SoftLine,
    /// Always a newline (forces the enclosing group to break).
    HardLine,
    /// Increase indentation for the contained doc.
    Indent(Box<Doc>),
    /// A group: render flat if it fits, otherwise broken.
    Group(Box<Doc>),
    /// Content that appears only in one mode. `break_` shows when the enclosing
    /// group is broken; `flat` shows when flat. The magic trailing comma is
    /// `IfBreak { break_: ",", flat: "" }`.
    IfBreak { break_: Box<Doc>, flat: Box<Doc> },
}

// Ergonomic constructors.
pub fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}
pub fn verbatim(s: impl Into<String>) -> Doc {
    Doc::Verbatim(s.into())
}
pub fn concat(docs: impl IntoIterator<Item = Doc>) -> Doc {
    Doc::Concat(docs.into_iter().collect())
}
pub fn group(d: Doc) -> Doc {
    Doc::Group(Box::new(d))
}
pub fn indent(d: Doc) -> Doc {
    Doc::Indent(Box::new(d))
}
pub fn if_break(break_: Doc, flat: Doc) -> Doc {
    Doc::IfBreak {
        break_: Box::new(break_),
        flat: Box::new(flat),
    }
}
/// A magic trailing comma: comma when broken, nothing when flat.
pub fn trailing_comma() -> Doc {
    if_break(text(","), text(""))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

/// Does the content fit in `width` columns if rendered flat?
///
/// Scans `next` (the group being measured, in flat mode) and then the `rest` of
/// the work stack — the commands that will print on the same line *after* the
/// group closes — until a newline would be emitted or the width budget is spent.
/// Threading `rest` is what makes this Wadler/Prettier-correct: a group must be
/// judged against the trailing same-line content, not in isolation, or it will
/// under-break when something follows it before the next hard line break.
///
/// `rest` mirrors `render`'s work stack (top = last element); `next` is pushed on
/// top so it is measured first, then `rest` is consumed in print order.
fn fits(mut width: isize, next: (usize, Mode, &Doc), rest: &[(usize, Mode, &Doc)]) -> bool {
    let mut work: Vec<(usize, Mode, &Doc)> = rest.to_vec();
    work.push(next);
    while let Some((ind, mode, doc)) = work.pop() {
        if width < 0 {
            return false;
        }
        match doc {
            Doc::Text(s) => width -= s.chars().count() as isize,
            Doc::Verbatim(s) => {
                // An embedded newline ends the current line, so the rest fits.
                if s.contains('\n') {
                    return true;
                }
                width -= s.chars().count() as isize;
            }
            Doc::Concat(ds) => {
                for d in ds.iter().rev() {
                    work.push((ind, mode, d));
                }
            }
            Doc::Indent(d) => work.push((ind + INDENT, mode, d)),
            Doc::Group(d) => work.push((ind, Mode::Flat, d)),
            Doc::Line => match mode {
                Mode::Flat => width -= 1,
                Mode::Break => return true, // a newline ends the line — it fits
            },
            Doc::SoftLine => match mode {
                Mode::Flat => {}
                Mode::Break => return true,
            },
            Doc::HardLine => return true,
            Doc::IfBreak { break_, flat } => match mode {
                Mode::Flat => work.push((ind, mode, flat)),
                Mode::Break => work.push((ind, mode, break_)),
            },
        }
    }
    width >= 0
}

/// Render a doc at the given max line `width`.
pub fn render(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    let mut col: usize = 0;
    // Work stack of (indent, mode, doc); top is the last element.
    let mut stack: Vec<(usize, Mode, &Doc)> = vec![(0, Mode::Break, doc)];

    while let Some((ind, mode, doc)) = stack.pop() {
        match doc {
            Doc::Text(s) => {
                out.push_str(s);
                col += s.chars().count();
            }
            Doc::Verbatim(s) => {
                out.push_str(s);
                // Column resumes after the last embedded newline (no re-indent).
                col = match s.rfind('\n') {
                    Some(i) => s[i + 1..].chars().count(),
                    None => col + s.chars().count(),
                };
            }
            Doc::Concat(ds) => {
                for d in ds.iter().rev() {
                    stack.push((ind, mode, d));
                }
            }
            Doc::Indent(d) => stack.push((ind + INDENT, mode, d)),
            Doc::Group(d) => {
                // Decide this group's mode by trying it flat *together with* the
                // content that will share its line afterwards (the rest of the
                // work stack), up to the next hard break. See `fits`.
                let flat_fits = fits(
                    width as isize - col as isize,
                    (ind, Mode::Flat, d),
                    &stack,
                );
                let m = if flat_fits { Mode::Flat } else { Mode::Break };
                stack.push((ind, m, d));
            }
            Doc::Line => match mode {
                Mode::Flat => {
                    out.push(' ');
                    col += 1;
                }
                Mode::Break => {
                    out.push('\n');
                    out.push_str(&indent_str(ind));
                    col = ind;
                }
            },
            Doc::SoftLine => match mode {
                Mode::Flat => {}
                Mode::Break => {
                    out.push('\n');
                    out.push_str(&indent_str(ind));
                    col = ind;
                }
            },
            Doc::HardLine => {
                out.push('\n');
                out.push_str(&indent_str(ind));
                col = ind;
            }
            Doc::IfBreak { break_, flat } => match mode {
                Mode::Flat => stack.push((ind, mode, flat)),
                Mode::Break => stack.push((ind, mode, break_)),
            },
        }
    }
    out
}

/// Build the doc for a function call `callee(arg0, arg1, ...)` with black-style
/// wrapping: flat when it fits, else one argument per line with a magic trailing
/// comma. This is the Phase 0 wrapping test case.
pub fn call(callee: &str, args: &[&str]) -> Doc {
    if args.is_empty() {
        return concat([text(callee), text("("), text(")")]);
    }
    let mut inner: Vec<Doc> = vec![Doc::SoftLine];
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            inner.push(text(","));
            inner.push(Doc::Line);
        }
        inner.push(text(*a));
    }
    inner.push(trailing_comma());
    group(concat([
        text(callee),
        text("("),
        indent(concat(inner)),
        Doc::SoftLine,
        text(")"),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_call_stays_flat() {
        let d = call("move", &["x", "y"]);
        assert_eq!(render(&d, 100), "move(x, y)");
    }

    #[test]
    fn empty_call() {
        assert_eq!(render(&call("ready", &[]), 100), "ready()");
    }

    #[test]
    fn long_call_wraps_one_per_line_with_trailing_comma() {
        // This argument list is far longer than 100 columns when flat.
        let args = [
            "first_long_argument_name",
            "second_long_argument_name",
            "third_long_argument_name",
            "fourth_long_argument_name",
        ];
        let d = call("configure_the_whole_system", &args);
        let out = render(&d, 100);
        let expected = "\
configure_the_whole_system(
\tfirst_long_argument_name,
\tsecond_long_argument_name,
\tthird_long_argument_name,
\tfourth_long_argument_name,
)";
        assert_eq!(out, expected, "\n--- got ---\n{out}\n");
    }

    #[test]
    fn boundary_respects_width() {
        // Fits in 100 but not in 20 → same doc, width drives the decision.
        let d = call("foo", &["alpha", "beta", "gamma"]);
        assert_eq!(render(&d, 100), "foo(alpha, beta, gamma)");
        assert_eq!(
            render(&d, 20),
            "foo(\n\talpha,\n\tbeta,\n\tgamma,\n)"
        );
    }

    /// A nested group must break when the *combined* line — the group plus the
    /// text that follows it on the same line — overflows the width, even though
    /// the group fits in isolation. Before threading the trailing content into
    /// `fits`, this under-broke: the inner group was judged flat (15 cols) and the
    /// 54-col combined line silently overflowed the 30-col budget.
    #[test]
    fn nested_group_breaks_for_trailing_same_line_content() {
        let inner = call("foo", &["aaaa", "bbbb"]); // "foo(aaaa, bbbb)" == 15 cols flat
        let doc = concat([inner, text(" == some_long_trailing_comparison_value")]);

        // Flat, the whole line is 54 cols — comfortably over 30. The inner group
        // alone is 15 cols and would "fit" if measured in isolation (the bug).
        let out = render(&doc, 30);
        let expected = "\
foo(
\taaaa,
\tbbbb,
) == some_long_trailing_comparison_value";
        assert_eq!(out, expected, "\n--- got ---\n{out}\n");
        assert!(
            out.contains('\n'),
            "inner group must break; trailing content pushes the line past 30 cols"
        );
    }

    /// The trailing content only forces a break when it actually overflows. With a
    /// wide enough budget the same doc stays flat — confirming the fix didn't just
    /// make groups break unconditionally.
    #[test]
    fn nested_group_stays_flat_when_trailing_content_fits() {
        let inner = call("foo", &["aaaa", "bbbb"]);
        let doc = concat([inner, text(" == some_long_trailing_comparison_value")]);
        assert_eq!(
            render(&doc, 100),
            "foo(aaaa, bbbb) == some_long_trailing_comparison_value"
        );
    }

    /// Idempotency at the IR level: rendering is a pure function of (doc, width),
    /// so repeated renders are byte-identical. (Full source idempotency also needs
    /// the CST->Doc lowering, which is Phase 1.)
    #[test]
    fn render_is_deterministic() {
        let d = call("configure_the_whole_system", &[
            "first_long_argument_name",
            "second_long_argument_name",
            "third_long_argument_name",
        ]);
        let a = render(&d, 100);
        let b = render(&d, 100);
        assert_eq!(a, b);
    }
}
