//! gdstrict-format: the formatter ("black for Godot").
//!
//! Phase 0 contained only the layout engine (`doc`). Phase 1 adds CST -> Doc
//! lowering (`lower`) on top of `gdstrict-syntax`: parse source to a tree-sitter
//! CST, walk it emitting the document IR, then render at the target line width.
//! Comment and blank-line preservation is handled by `trivia`, which the block
//! lowering uses to attach comments to the statements they belong to.

pub mod doc;
pub mod lower;
pub mod trivia;

/// Default line width (Godot style guide / black default).
pub const DEFAULT_WIDTH: usize = 100;

/// Format GDScript source at the default line width.
pub fn format(source: &str) -> String {
    format_width(source, DEFAULT_WIDTH)
}

/// Format GDScript source at an explicit line width.
pub fn format_width(source: &str, width: usize) -> String {
    let tree = gdstrict_syntax::parse(source);
    let d = lower::lower(tree.root_node(), source);
    let mut out = doc::render(&d, width);
    // Canonical files end in exactly one newline.
    while out.ends_with('\n') {
        out.pop();
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Format, then assert the result re-parses with zero defects and is
    /// idempotent (`format(format(x)) == format(x)`) — the two hard invariants
    /// from the plan (output still parses; double-format is a no-op).
    fn check_roundtrip(src: &str) -> String {
        let once = format(src);
        assert!(
            gdstrict_syntax::parses_clean(&once),
            "formatted output has parse defects:\n{once}\ndefects: {:?}",
            gdstrict_syntax::defects(&gdstrict_syntax::parse(&once))
        );
        let twice = format(&once);
        assert_eq!(once, twice, "not idempotent\n--- once ---\n{once}\n--- twice ---\n{twice}");
        once
    }

    #[test]
    fn empty_input() {
        assert_eq!(format(""), "");
    }

    #[test]
    fn simple_statements() {
        let out = check_roundtrip("extends Node\nvar x:int=5\n");
        assert_eq!(out, "extends Node\nvar x: int = 5\n");
    }

    #[test]
    fn inferred_var() {
        assert_eq!(check_roundtrip("var x:=5\n"), "var x := 5\n");
    }

    #[test]
    fn function_with_body() {
        let out = check_roundtrip("func _ready()->void:\n\tpass\n");
        assert_eq!(out, "func _ready() -> void:\n\tpass\n");
    }

    #[test]
    fn call_stays_flat_when_short() {
        let out = check_roundtrip("func f()->void:\n\tprint(1, 2, 3)\n");
        assert_eq!(out, "func f() -> void:\n\tprint(1, 2, 3)\n");
    }

    #[test]
    fn long_call_wraps_with_trailing_comma() {
        let src = "func f() -> void:\n\tconfigure(the_first_long_argument_value, the_second_long_argument_value, the_third_long_argument_value)\n";
        let out = check_roundtrip(src);
        let expected = "\
func f() -> void:
\tconfigure(
\t\tthe_first_long_argument_value,
\t\tthe_second_long_argument_value,
\t\tthe_third_long_argument_value,
\t)
";
        assert_eq!(out, expected, "\n--- got ---\n{out}");
    }

    #[test]
    fn typed_collections_roundtrip() {
        // The Dictionary[K, V] syntax that broke gdformat (issue #363).
        check_roundtrip(
            "var tweens: Dictionary[String, Tween] = {}\nvar grid: Array[Array[int]] = []\n",
        );
    }

    #[test]
    fn array_and_dict_literals() {
        let out = check_roundtrip("var a := [1, 2, 3]\nvar d := {1: \"one\", 2: \"two\"}\n");
        assert_eq!(out, "var a := [1, 2, 3]\nvar d := {1: \"one\", 2: \"two\"}\n");
    }

    #[test]
    fn if_elif_else() {
        let src = "func f(n: int) -> void:\n\tif n > 0:\n\t\tprint(1)\n\telif n < 0:\n\t\tprint(2)\n\telse:\n\t\tprint(3)\n";
        let out = check_roundtrip(src);
        let expected = "\
func f(n: int) -> void:
\tif n > 0:
\t\tprint(1)
\telif n < 0:
\t\tprint(2)
\telse:
\t\tprint(3)
";
        assert_eq!(out, expected);
    }

    #[test]
    fn blank_lines_collapse_to_one() {
        let out = check_roundtrip("var a := 1\n\n\n\nvar b := 2\n");
        assert_eq!(out, "var a := 1\n\nvar b := 2\n");
    }

    #[test]
    fn preserved_blank_line_has_no_trailing_whitespace() {
        // Blank lines inside an indented block must not carry indent spaces.
        let out = check_roundtrip("func f() -> void:\n\tvar a := 1\n\n\tvar b := 2\n");
        assert_eq!(out, "func f() -> void:\n\tvar a := 1\n\n\tvar b := 2\n");
        for line in out.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in line: {line:?}");
        }
    }

    #[test]
    fn multiline_string_preserved_byte_exact() {
        let src = "var t := \"\"\"\n\tmulti\n\tline\n\"\"\"\n";
        let out = check_roundtrip(src);
        assert!(out.contains("\"\"\"\n\tmulti\n\tline\n\"\"\""), "got:\n{out}");
    }

    #[test]
    fn while_break_continue() {
        check_roundtrip(
            "func f() -> void:\n\tvar i := 0\n\twhile i < 10:\n\t\tif i == 5:\n\t\t\tbreak\n\t\tcontinue\n",
        );
    }

    #[test]
    fn nested_array_wraps_one_per_line() {
        let src = "var big := [aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, cccccccccccccc, dddddddddddddd, eeeeeeeeeeeeee, ffffffffffffff]\n";
        let out = check_roundtrip(src);
        assert!(out.contains("[\n\taaaaaaaaaaaaaa,\n"), "got:\n{out}");
        assert!(out.ends_with("\tffffffffffffff,\n]\n"), "got:\n{out}");
    }

    #[test]
    fn empty_collections_and_unary() {
        let out = check_roundtrip("var d := {}\nvar e := []\nvar n := not true\n");
        assert_eq!(out, "var d := {}\nvar e := []\nvar n := not true\n");
    }

    #[test]
    fn property_with_get_set_roundtrips() {
        // Property accessor bodies use the verbatim fallback; they must still
        // re-parse and be idempotent.
        check_roundtrip(
            "var hp := 100:\n\tget:\n\t\treturn hp\n\tset(value):\n\t\thp = value\n",
        );
    }

    // --- comment & blank-line trivia (integration over the real lowering) ----
    //
    // These drive comments through the full `format()` — real expression layout,
    // not a verbatim stand-in — and assert each comment survives, lands in the
    // right place, and the output re-parses (`check_roundtrip`). This is the
    // gdformat `CommentPersistenceViolation` regression surface.

    #[test]
    fn leading_comment_on_func_survives() {
        // The comment stays on its own line directly above the func, and the func
        // body is still reformatted (`print( x )` -> `print(x)`).
        let out = check_roundtrip("# explains the function\nfunc foo():\n\tprint( x )\n");
        let lines: Vec<&str> = out.lines().collect();
        let ci = lines.iter().position(|l| l.contains("# explains")).unwrap();
        assert!(
            lines[ci + 1].starts_with("func foo"),
            "leading comment must sit directly above the func; got:\n{out}"
        );
        assert!(out.contains("\tprint(x)"), "body must be reformatted; got:\n{out}");
    }

    #[test]
    fn inline_comment_stays_on_statement_line_and_expr_is_formatted() {
        // The inline comment must NOT be relocated to its own line, and the
        // statement it trails is still normalized (`var x:int=5`).
        let out = check_roundtrip("func foo():\n\tvar x:int=5  # the counter\n");
        let line = out.lines().find(|l| l.contains("var x")).expect("var line present");
        assert_eq!(
            line, "\tvar x: int = 5  # the counter",
            "inline comment must stay inline with the reformatted statement; got:\n{out}"
        );
    }

    #[test]
    fn doc_comment_block_survives_in_order() {
        let out = check_roundtrip("## A documented class.\n## Second doc line.\nclass_name Foo\n");
        let a = out.find("## A documented").expect("doc line 1 present");
        let b = out.find("## Second doc").expect("doc line 2 present");
        let c = out.find("class_name Foo").expect("decl present");
        assert!(a < b && b < c, "doc block order must be preserved; got:\n{out}");
    }

    #[test]
    fn blank_run_between_members_collapses_with_comment_kept() {
        // A run of blanks between two members collapses to one, and a leading
        // comment on the second member is preserved across the gap.
        let out = check_roundtrip("var a:=1\n\n\n\n# about b\nvar b:=2\n");
        assert_eq!(
            out, "var a := 1\n\n# about b\nvar b := 2\n",
            "blank run should collapse to one and keep the comment; got:\n{out}"
        );
    }

    #[test]
    fn in_body_comment_keeps_indentation() {
        let out = check_roundtrip("func foo():\n\t# step one\n\tvar x:=1\n");
        let lines: Vec<&str> = out.lines().collect();
        let ci = lines.iter().position(|l| l.contains("# step one")).unwrap();
        assert!(
            lines[ci].starts_with("\t#"),
            "in-body comment must be indented to the block; got:\n{out}"
        );
        assert!(lines[ci + 1].contains("var x := 1"));
    }

    #[test]
    fn dangling_comment_at_end_of_body_survives() {
        let out = check_roundtrip("func foo():\n\tpass\n\t# trailing block comment\n");
        assert!(
            out.contains("# trailing block comment"),
            "dangling comment must not be dropped; got:\n{out}"
        );
    }

    #[test]
    fn mixed_trivia_round_trips_and_keeps_every_comment() {
        let src = "\
# top of file
extends Node


## Doc for greeting.
func greet(name):
\t# build the message
\tvar msg=\"hi\"  # placeholder
\tprint( msg )
";
        let out = check_roundtrip(src);
        for needle in [
            "# top of file",
            "## Doc for greeting.",
            "# build the message",
            "# placeholder",
        ] {
            assert!(out.contains(needle), "lost {needle:?} in:\n{out}");
        }
        let msg_line = out.lines().find(|l| l.contains("var msg")).unwrap();
        assert_eq!(
            msg_line, "\tvar msg = \"hi\"  # placeholder",
            "inline comment stays inline and the assignment is normalized; got:\n{out}"
        );
    }

    /// Every grammar fixture must format, re-parse clean, and be idempotent.
    #[test]
    fn all_grammar_fixtures_roundtrip() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/grammar");
        let mut count = 0;
        for entry in std::fs::read_dir(dir).expect("read fixtures/grammar dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("gd") {
                continue;
            }
            count += 1;
            let src = std::fs::read_to_string(&path).unwrap();
            let once = format(&src);
            assert!(
                gdstrict_syntax::parses_clean(&once),
                "{}: formatted output has parse defects:\n{once}\ndefects: {:?}",
                path.display(),
                gdstrict_syntax::defects(&gdstrict_syntax::parse(&once))
            );
            let twice = format(&once);
            assert_eq!(
                once,
                twice,
                "{}: not idempotent\n--- once ---\n{once}\n--- twice ---\n{twice}",
                path.display()
            );
        }
        assert!(count > 0, "no fixtures found");
    }
}
