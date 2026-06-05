//! CST -> Doc lowering: walk the tree-sitter-gdscript CST and emit the layout IR.
//!
//! This is the bulk of the formatter. Every node kind is mapped to a [`Doc`]
//! built from the existing primitives in [`crate::doc`] — `Group`, `Indent`,
//! `Line`/`SoftLine`/`HardLine`, and `IfBreak`. Wrappable constructs (call
//! argument lists, arrays, dictionaries, parameter lists, enum bodies) reuse the
//! black-style [`delimited`] helper, which is the same shape as [`crate::doc::call`]:
//! flat when it fits, one element per line with a magic trailing comma otherwise.
//!
//! Robustness contract: any node kind not given bespoke handling falls through to
//! [`leaf`], which emits the node's exact source bytes. That guarantees every
//! subtree round-trips even before it gets pretty-printing, so the formatter never
//! corrupts code it doesn't yet understand.

use crate::doc::{concat, group, indent, text, trailing_comma, verbatim, Doc};
use crate::trivia::{attach, Comment, Element, Item, RawElement};
use tree_sitter::Node;

/// Exact source bytes of a node, as a `Doc`. Uses [`Doc::Verbatim`] when the
/// slice spans multiple lines (e.g. a triple-quoted string) so it is reproduced
/// byte-for-byte; otherwise a plain [`Doc::Text`].
fn leaf(src: &str, n: Node) -> Doc {
    let s = slice(src, n);
    if s.contains('\n') {
        verbatim(s)
    } else {
        text(s)
    }
}

fn slice<'a>(src: &'a str, n: Node) -> &'a str {
    &src[n.start_byte()..n.end_byte()]
}

fn named_children<'a>(n: Node<'a>) -> Vec<Node<'a>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

/// The anonymous (operator/keyword) tokens of a node, joined by spaces.
/// For a `binary_operator` this yields `+`, `==`, `and`, `not in`, `as`, …;
/// `line_continuation`/`comment` are *named* nodes and are excluded.
fn operator(src: &str, n: Node) -> String {
    let mut c = n.walk();
    n.children(&mut c)
        .filter(|ch| !ch.is_named())
        .map(|ch| slice(src, ch))
        .collect::<Vec<_>>()
        .join(" ")
}

fn field<'a>(n: Node<'a>, name: &str) -> Option<Node<'a>> {
    n.child_by_field_name(name)
}

// --- black-style wrapping list (mirrors doc::call) -------------------------

/// `open` items `close` with black wrapping: flat if it fits, else one item per
/// line with a magic trailing comma. This is the general-purpose version of the
/// Phase 0 [`crate::doc::call`] fixture (which stays as a fixed `&str`-based test
/// case); real lowering goes through here. Keep the two in sync if the wrapping
/// shape ever changes.
///
/// `force_break` implements Prettier's "magic trailing comma": when the *input*
/// already had a trailing comma before `close` (see [`has_magic_trailing_comma`]),
/// the caller passes `true` and the collection stays expanded — one item per line —
/// regardless of whether it would otherwise fit flat. The expanded shape is
/// byte-identical to the soft path's broken mode (same `open` / one-item-per-line /
/// trailing comma / dedented `close`), so output stays idempotent: a re-parse sees
/// the emitted trailing comma and force-breaks again to the same bytes.
fn delimited(open: &str, items: Vec<Doc>, close: &str, force_break: bool) -> Doc {
    if items.is_empty() {
        return concat([text(open), text(close)]);
    }
    if force_break {
        // Build the broken form directly with `HardLine`s instead of a fits()-driven
        // `Group`, so it always expands. The trailing comma is unconditional here
        // (`text(",")`) rather than `trailing_comma()` since there is no flat mode.
        let inner = interleave(items, Doc::HardLine, Doc::HardLine, text(","));
        return concat([text(open), indent(concat(inner)), Doc::HardLine, text(close)]);
    }
    let inner = interleave(items, Doc::SoftLine, Doc::Line, trailing_comma());
    group(concat([text(open), indent(concat(inner)), Doc::SoftLine, text(close)]))
}

/// Interleave wrappable list `items` into the inner doc sequence shared by both
/// of [`delimited`]'s shapes: a `lead` break after the opener, a `,` + `sep` break
/// between items, and a `term` after the last item (a real comma when force-broken,
/// a magic [`trailing_comma`] in the soft path). Only the break/terminator
/// primitives differ between the two shapes — the interleaving is identical.
fn interleave(items: Vec<Doc>, lead: Doc, sep: Doc, term: Doc) -> Vec<Doc> {
    let mut inner: Vec<Doc> = vec![lead];
    for (i, it) in items.into_iter().enumerate() {
        if i > 0 {
            inner.push(text(","));
            inner.push(sep.clone());
        }
        inner.push(it);
    }
    inner.push(term);
    inner
}

/// Whether a collection node (`array`, `dictionary`, `arguments`) carries an
/// explicit trailing comma in the *input* source — a `,` token immediately before
/// the closing delimiter, skipping any interleaved comments. Prettier treats this
/// "magic trailing comma" as an author's request to keep the collection expanded.
///
/// Scans the node's children (including the anonymous delimiter/comma tokens, which
/// `named_children` omits) from the end: skip the closing delimiter, skip comments,
/// then the next token decides — a `,` means a trailing comma, an element means none.
fn has_magic_trailing_comma(n: Node) -> bool {
    let mut c = n.walk();
    let children: Vec<Node> = n.children(&mut c).collect();
    let mut iter = children.iter().rev();
    iter.next(); // closing delimiter (last child)
    for ch in iter {
        match ch.kind() {
            "comment" => continue,
            "," => return true,
            _ => return false,
        }
    }
    false
}

/// `header:` followed by an indented body block. Used by every compound
/// statement (func, class, if/elif/else, for, while, match arm, lambda).
fn suite(header: Doc, body: Node, src: &str) -> Doc {
    concat([header, text(":"), indent(block(body, src, true))])
}

/// Lower a block of statements (`body`, `class_body`, `source`, `match_body`).
///
/// `leading` controls whether a `HardLine` is emitted *before* the first
/// statement — true inside an indented suite (the newline after the `:`), false
/// at file scope. Blank lines are normalized to the GDScript style guide: at most
/// one blank line inside a block, but up to two between adjacent *top-level
/// definitions* (functions / classes) at file scope. Leading and trailing blank
/// lines are dropped (the first item never gets a preceding blank; `format()`
/// trims the file's trailing newlines). See [`blank_cap`].
///
/// Comments are interleaved `(comment)` children here; rather than lowering each
/// on its own line (which relocates inline comments and is exactly where gdformat
/// breaks), we hand the child sequence to [`crate::trivia::attach`] so each
/// comment is bound to the statement it belongs to — leading (own line above),
/// trailing (inline, same row), or dangling (kept when no statement follows).
fn block(node: Node, src: &str, leading: bool) -> Doc {
    let raw: Vec<RawElement<Node>> = named_children(node)
        .into_iter()
        .map(|kid| {
            let element = if kid.kind() == "comment" {
                Element::Comment(Comment::parse(slice(src, kid)))
            } else {
                Element::Stmt(kid)
            };
            RawElement {
                element,
                start_row: kid.start_position().row,
                end_row: kid.end_position().row,
            }
        })
        .collect();

    let items = attach(raw);
    let mut parts: Vec<Doc> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i == 0 {
            if leading {
                parts.push(Doc::HardLine);
            }
        } else {
            // Normalize the blank run: cap at 1 in general, 2 between top-level
            // definitions. Each preserved blank is a bare newline (no indent) so it
            // carries no trailing whitespace; the following `HardLine`
            // breaks+indents to the item.
            let cap = blank_cap(&items[i - 1], item, leading);
            for _ in 0..item.blanks_before.min(cap) {
                parts.push(verbatim("\n"));
            }
            parts.push(Doc::HardLine);
        }
        parts.push(item_doc(item, src));
    }
    concat(parts)
}

/// How many blank lines to keep between `prev` and `cur`. The GDScript style
/// guide surrounds top-level functions and class definitions with two blank
/// lines; everything else (statements, members, block bodies) gets at most one.
///
/// `leading` is false only at file scope (the `source` block), so the two-blank
/// allowance is restricted to top-level definitions — methods inside an inner
/// `class` body keep the conservative single-blank cap.
fn blank_cap(prev: &Item<Node<'_>>, cur: &Item<Node<'_>>, leading: bool) -> usize {
    if !leading && (is_top_level_def(prev) || is_top_level_def(cur)) {
        2
    } else {
        1
    }
}

/// Whether an item's statement is a definition the style guide surrounds with two
/// blank lines: a function, constructor, or (inner) class.
fn is_top_level_def(item: &Item<Node<'_>>) -> bool {
    item.stmt.is_some_and(|s| {
        matches!(
            s.kind(),
            "function_definition" | "constructor_definition" | "class_definition"
        )
    })
}

/// Lower one attached item: its leading comments (each on its own line), the
/// statement (via the normal [`lower`] dispatch — no expression re-layout is
/// regressed), and any trailing inline comment. A dangling comment group (no
/// statement) emits just its comments.
fn item_doc(item: &Item<Node<'_>>, src: &str) -> Doc {
    let mut lines: Vec<Doc> = item.leading.iter().map(|c| text(c.text.clone())).collect();
    if let Some(stmt) = item.stmt {
        let mut stmt_doc = lower(stmt, src);
        if let Some(trailing) = &item.trailing {
            // Inline comment stays on the statement's last line after a two-space
            // gap (black-family convention). doc.rs has no line-suffix primitive,
            // so append directly; if the statement's layout breaks, this lands the
            // comment after the closing delimiter — still the statement's last line.
            stmt_doc = concat([stmt_doc, text(format!("  {}", trailing.text))]);
        }
        lines.push(stmt_doc);
    }
    join(lines, Doc::HardLine)
}

/// Render the `annotations` child (if any) of a statement, each annotation
/// followed by a space when it sat on the same source row as `ref_row`
/// (e.g. `@export var x`) or a `HardLine` when it was on its own line above
/// (e.g. `@warning_ignore(...)` over a function). Returns an empty concat when
/// there are no annotations.
fn annotations_prefix(node: Node, ref_row: usize, src: &str) -> Doc {
    let Some(annos) = named_children(node)
        .into_iter()
        .find(|c| c.kind() == "annotations")
    else {
        return concat([]);
    };
    let mut parts = Vec::new();
    for a in named_children(annos) {
        parts.push(lower(a, src));
        if a.end_position().row == ref_row {
            parts.push(text(" "));
        } else {
            parts.push(Doc::HardLine);
        }
    }
    concat(parts)
}

// --- the dispatch ----------------------------------------------------------

/// Lower any node to a [`Doc`]. The single entry point for both statements and
/// expressions — GDScript node kinds are disjoint, so one match covers both.
pub fn lower(n: Node, src: &str) -> Doc {
    match n.kind() {
        // ---- leaves / atoms ----
        "identifier" | "name" | "integer" | "float" | "string" | "string_name" | "node_path"
        | "get_node" | "true" | "false" | "null" | "comment" | "static_keyword"
        | "region_start" | "region_end" | "region_label" | "pattern_open_ending"
        | "escape_sequence" | "line_continuation" | "breakpoint_statement" | "pass_statement"
        | "break_statement" | "continue_statement" => leaf(src, n),

        // A `type` wraps a single identifier / subscript / inferred_type.
        "type" => named_children(n)
            .first()
            .map(|c| lower(*c, src))
            .unwrap_or_else(|| leaf(src, n)),
        "inferred_type" => text(":="),

        // ---- file / blocks ----
        "source" => {
            // Trailing newline is added by the caller (format()).
            block(n, src, false)
        }
        "body" | "class_body" | "match_body" => block(n, src, true),

        // ---- statements ----
        "expression_statement" => named_children(n)
            .first()
            .map(|c| lower(*c, src))
            .unwrap_or_else(|| leaf(src, n)),

        "extends_statement" => {
            // `extends Foo` — the type is the sole named child.
            match named_children(n).first() {
                Some(t) => concat([text("extends "), lower(*t, src)]),
                None => leaf(src, n),
            }
        }
        "class_name_statement" => match field(n, "name") {
            Some(name) => concat([text("class_name "), lower(name, src)]),
            None => leaf(src, n),
        },

        "variable_statement" | "const_statement" | "export_variable_statement"
        | "onready_variable_statement" => variable_like(n, src),

        "function_definition" | "constructor_definition" => def(n, src),
        "lambda" => def(n, src),

        "signal_statement" => signal(n, src),
        "class_definition" => class_def(n, src),
        "enum_definition" => enum_def(n, src),

        "if_statement" => if_stmt(n, src),
        "elif_clause" => suite(
            concat([text("elif "), expr_field(n, "condition", src)]),
            field(n, "body").unwrap(),
            src,
        ),
        "else_clause" => suite(text("else"), field(n, "body").unwrap(), src),

        "for_statement" => for_stmt(n, src),
        "while_statement" => suite(
            concat([text("while "), expr_field(n, "condition", src)]),
            field(n, "body").unwrap(),
            src,
        ),
        "match_statement" => suite(
            concat([text("match "), expr_field(n, "value", src)]),
            field(n, "body").unwrap(),
            src,
        ),
        "pattern_section" => pattern_section(n, src),

        "return_statement" => match named_children(n).first() {
            Some(e) => concat([text("return "), lower(*e, src)]),
            None => text("return"),
        },

        "annotation" => annotation(n, src),

        // ---- expressions ----
        "parenthesized_expression" => match named_children(n).first() {
            Some(e) => concat([text("("), lower(*e, src), text(")")]),
            None => leaf(src, n),
        },
        "array" => delimited(
            "[",
            named_children(n).iter().map(|c| lower(*c, src)).collect(),
            "]",
            has_magic_trailing_comma(n),
        ),
        "dictionary" => delimited(
            "{",
            named_children(n).iter().map(|c| lower(*c, src)).collect(),
            "}",
            has_magic_trailing_comma(n),
        ),
        "pair" => pair(n, src),
        "arguments" => delimited(
            "(",
            named_children(n).iter().map(|c| lower(*c, src)).collect(),
            ")",
            has_magic_trailing_comma(n),
        ),
        "parameters" => delimited(
            "(",
            named_children(n).iter().map(|c| param(*c, src)).collect(),
            ")",
            false,
        ),

        "call" => call_like(n, src),
        "attribute_call" => call_like(n, src),
        "subscript" | "attribute_subscript" => subscript(n, src),
        "subscript_arguments" => {
            // Type args / index — kept flat (`Dictionary[String, int]`, `a[0]`).
            let items: Vec<Doc> = named_children(n).iter().map(|c| lower(*c, src)).collect();
            join(items, text(", "))
        }
        "attribute" => join(named_children(n).iter().map(|c| lower(*c, src)).collect(), text(".")),

        "binary_operator" => concat([
            expr_field(n, "left", src),
            text(" "),
            text(operator(src, n)),
            text(" "),
            expr_field(n, "right", src),
        ]),
        "unary_operator" => unary(n, src),
        "conditional_expression" => concat([
            expr_field(n, "left", src),
            text(" if "),
            expr_field(n, "condition", src),
            text(" else "),
            expr_field(n, "right", src),
        ]),
        "await_expression" => match named_children(n).first() {
            Some(e) => concat([text("await "), lower(*e, src)]),
            None => leaf(src, n),
        },

        "assignment" => concat([
            expr_field(n, "left", src),
            text(" = "),
            expr_field(n, "right", src),
        ]),
        "augmented_assignment" => concat([
            expr_field(n, "left", src),
            text(" "),
            text(operator(src, n)),
            text(" "),
            expr_field(n, "right", src),
        ]),

        // ---- match patterns ----
        "pattern_binding" => match named_children(n).first() {
            Some(name) => concat([text("var "), lower(*name, src)]),
            None => leaf(src, n),
        },
        "pattern_guard" => match named_children(n).first() {
            Some(e) => concat([text("when "), lower(*e, src)]),
            None => leaf(src, n),
        },

        // ---- fallback: exact source bytes (always round-trips) ----
        _ => leaf(src, n),
    }
}

/// Lower the node in field `name`, or empty if absent.
fn expr_field(n: Node, name: &str, src: &str) -> Doc {
    match field(n, name) {
        Some(c) => lower(c, src),
        None => concat([]),
    }
}

fn join(items: Vec<Doc>, sep: Doc) -> Doc {
    let mut parts = Vec::new();
    for (i, it) in items.into_iter().enumerate() {
        if i > 0 {
            parts.push(sep.clone());
        }
        parts.push(it);
    }
    concat(parts)
}

/// The base/callee doc (the named child that is *not* the `arguments` field)
/// paired with the `arguments` field node, for `call`/`attribute_call`/
/// `subscript`/`attribute_subscript`.
fn base_and_args<'a>(n: Node<'a>, src: &str) -> (Doc, Option<Node<'a>>) {
    let args = field(n, "arguments");
    let base = named_children(n)
        .into_iter()
        .find(|c| Some(c.id()) != args.map(|a| a.id()));
    let base_doc = match base {
        Some(c) => lower(c, src),
        None => concat([]),
    };
    (base_doc, args)
}

/// `callee(args)` for `call` and `attribute_call` — first named child is the
/// callee/method, the `arguments` field is the (wrapping) argument list.
fn call_like(n: Node, src: &str) -> Doc {
    let (callee_doc, args) = base_and_args(n, src);
    let args_doc = match args {
        Some(a) => lower(a, src),
        None => text("()"),
    };
    concat([callee_doc, args_doc])
}

/// `base[index]` for `subscript` / `attribute_subscript`. Kept flat.
fn subscript(n: Node, src: &str) -> Doc {
    let (base_doc, args) = base_and_args(n, src);
    let args_doc = match args {
        Some(a) => lower(a, src),
        None => concat([]),
    };
    concat([base_doc, text("["), args_doc, text("]")])
}

fn unary(n: Node, src: &str) -> Doc {
    let op = operator(src, n);
    let operand = named_children(n)
        .into_iter()
        .find(|c| c.kind() != "comment");
    let sep = if op.chars().next().is_some_and(|c| c.is_alphabetic()) {
        " "
    } else {
        ""
    };
    match operand {
        Some(e) => concat([text(op), text(sep), lower(e, src)]),
        None => text(op),
    }
}

/// `key: value` (or Lua-style `key = value`). The separator is recovered from
/// the source gap between the key and value so both forms round-trip.
fn pair(n: Node, src: &str) -> Doc {
    let left = field(n, "left");
    let value = field(n, "value");
    match (left, value) {
        (Some(l), Some(v)) => {
            let gap = &src[l.end_byte()..v.start_byte()];
            let sep = if gap.contains('=') { " = " } else { ": " };
            concat([lower(l, src), text(sep), lower(v, src)])
        }
        _ => leaf(src, n),
    }
}

/// Shared lowering for `variable_statement` / `const_statement` and friends.
fn variable_like(n: Node, src: &str) -> Doc {
    // Property syntax (get/set bodies) is rare and intricate — reproduce it
    // verbatim rather than risk mangling it.
    if named_children(n).iter().any(|c| {
        matches!(
            c.kind(),
            "set_body" | "get_body" | "setget" | "setter" | "getter"
        )
    }) {
        return leaf(src, n);
    }

    let name = match field(n, "name") {
        Some(name) => name,
        None => return leaf(src, n),
    };
    let mut parts: Vec<Doc> = Vec::new();
    parts.push(annotations_prefix(n, name.start_position().row, src));

    if named_children(n).iter().any(|c| c.kind() == "static_keyword") {
        parts.push(text("static "));
    }
    let keyword = if n.kind() == "const_statement" {
        "const "
    } else {
        "var "
    };
    parts.push(text(keyword));
    parts.push(lower(name, src));

    let ty = field(n, "type");
    let value = field(n, "value");
    match (ty, value) {
        (Some(t), Some(v)) if t.kind() == "inferred_type" => {
            parts.push(text(" := "));
            parts.push(lower(v, src));
        }
        (Some(t), Some(v)) => {
            parts.push(text(": "));
            parts.push(lower(t, src));
            parts.push(text(" = "));
            parts.push(lower(v, src));
        }
        (Some(t), None) => {
            parts.push(text(": "));
            parts.push(lower(t, src));
        }
        (None, Some(v)) => {
            parts.push(text(" = "));
            parts.push(lower(v, src));
        }
        (None, None) => {}
    }
    concat(parts)
}

/// `func name(params) [-> Type][: body]`, also constructors (`func _init`) and
/// lambdas (`func` / `func name`). The keyword+name prefix is sliced straight
/// from source, which captures `static`/`func`/`_init` without special cases.
fn def(n: Node, src: &str) -> Doc {
    let params = match field(n, "parameters") {
        Some(p) => p,
        None => return leaf(src, n),
    };
    let prefix = src[n.start_byte()..params.start_byte()].trim_end();
    let mut head: Vec<Doc> = vec![text(prefix), lower(params, src)];
    if let Some(rt) = field(n, "return_type") {
        head.push(text(" -> "));
        head.push(lower(rt, src));
    }
    let header = concat(head);
    match field(n, "body") {
        Some(body) => suite(header, body, src),
        None => header, // abstract function: signature only, no `:`
    }
}

fn signal(n: Node, src: &str) -> Doc {
    let name = match field(n, "name") {
        Some(name) => name,
        None => return leaf(src, n),
    };
    let mut parts = vec![text("signal "), lower(name, src)];
    if let Some(params) = field(n, "parameters") {
        parts.push(lower(params, src));
    }
    concat(parts)
}

fn class_def(n: Node, src: &str) -> Doc {
    let name = match field(n, "name") {
        Some(name) => name,
        None => return leaf(src, n),
    };
    let body = match field(n, "body") {
        Some(b) => b,
        None => return leaf(src, n),
    };
    let mut header = vec![text("class "), lower(name, src)];
    if let Some(ext) = field(n, "extends") {
        header.push(text(" "));
        header.push(lower(ext, src));
    }
    suite(concat(header), body, src)
}

fn enum_def(n: Node, src: &str) -> Doc {
    let body = match field(n, "body") {
        Some(b) => b,
        None => return leaf(src, n),
    };
    let mut prefix = vec![text("enum ")];
    if let Some(name) = field(n, "name") {
        prefix.push(lower(name, src));
        prefix.push(text(" "));
    }
    let items: Vec<Doc> = named_children(body)
        .iter()
        .map(|c| enumerator(*c, src))
        .collect();
    // Scope magic-trailing-comma to array/dict/call (the issue's three cases); enum
    // bodies keep the current fits()-driven behavior.
    concat([concat(prefix), delimited("{", items, "}", false)])
}

fn enumerator(n: Node, src: &str) -> Doc {
    if n.kind() != "enumerator" {
        return leaf(src, n);
    }
    let left = field(n, "left").or_else(|| named_children(n).first().copied());
    let left_doc = left.map(|l| lower(l, src)).unwrap_or_else(|| leaf(src, n));
    match field(n, "right") {
        Some(v) => concat([left_doc, text(" = "), lower(v, src)]),
        None => left_doc,
    }
}

fn if_stmt(n: Node, src: &str) -> Doc {
    let cond = field(n, "condition").unwrap();
    let body = field(n, "body").unwrap();
    let mut parts = vec![suite(concat([text("if "), lower(cond, src)]), body, src)];
    for c in named_children(n) {
        match c.kind() {
            "elif_clause" | "else_clause" => {
                parts.push(Doc::HardLine);
                parts.push(lower(c, src));
            }
            _ => {}
        }
    }
    concat(parts)
}

fn for_stmt(n: Node, src: &str) -> Doc {
    let left = field(n, "left").unwrap();
    let right = field(n, "right").unwrap();
    let body = field(n, "body").unwrap();
    let mut header = vec![text("for "), lower(left, src)];
    if let Some(ty) = field(n, "type") {
        header.push(text(": "));
        header.push(lower(ty, src));
    }
    header.push(text(" in "));
    header.push(lower(right, src));
    suite(concat(header), body, src)
}

fn pattern_section(n: Node, src: &str) -> Doc {
    let body = field(n, "body").unwrap();
    let mut patterns: Vec<Doc> = Vec::new();
    let mut guard: Option<Doc> = None;
    for c in named_children(n) {
        match c.kind() {
            "pattern_guard" => guard = Some(lower(c, src)),
            _ if Some(c.id()) == Some(body.id()) => {}
            "comment" => {}
            _ => patterns.push(lower(c, src)),
        }
    }
    let mut header = vec![join(patterns, text(", "))];
    if let Some(g) = guard {
        header.push(text(" "));
        header.push(g);
    }
    suite(concat(header), body, src)
}

fn annotation(n: Node, src: &str) -> Doc {
    // `@name` or `@name(args)` — no spaces.
    let ident = named_children(n)
        .into_iter()
        .find(|c| c.kind() == "identifier");
    let mut parts = vec![text("@")];
    match ident {
        Some(id) => parts.push(lower(id, src)),
        None => return leaf(src, n),
    }
    if let Some(args) = field(n, "arguments") {
        parts.push(lower(args, src));
    }
    concat(parts)
}

/// A single parameter inside a `parameters` list.
fn param(n: Node, src: &str) -> Doc {
    match n.kind() {
        "typed_parameter" => {
            let name = named_children(n).first().copied();
            let ty = field(n, "type");
            match (name, ty) {
                (Some(nm), Some(t)) => concat([lower(nm, src), text(": "), lower(t, src)]),
                _ => leaf(src, n),
            }
        }
        "typed_default_parameter" => {
            let name = named_children(n).first().copied();
            let ty = field(n, "type");
            let value = field(n, "value");
            match (name, ty, value) {
                (Some(nm), Some(t), Some(v)) if t.kind() == "inferred_type" => {
                    concat([lower(nm, src), text(" := "), lower(v, src)])
                }
                (Some(nm), Some(t), Some(v)) => concat([
                    lower(nm, src),
                    text(": "),
                    lower(t, src),
                    text(" = "),
                    lower(v, src),
                ]),
                _ => leaf(src, n),
            }
        }
        // `identifier` (bare), `default_parameter`, anything else: single line,
        // reproduced verbatim.
        _ => leaf(src, n),
    }
}
