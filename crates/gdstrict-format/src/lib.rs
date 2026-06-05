//! gdstrict-format: the formatter ("black for Godot").
//!
//! Phase 0 contained only the layout engine (`doc`). Phase 1 adds CST -> Doc
//! lowering (`lower`) on top of `gdstrict-syntax`: parse source to a tree-sitter
//! CST, walk it emitting the document IR, then render at the target line width.

pub mod doc;
pub mod lower;

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
        assert_eq!(out, "func _ready() -> void:\n    pass\n");
    }

    #[test]
    fn call_stays_flat_when_short() {
        let out = check_roundtrip("func f()->void:\n\tprint(1, 2, 3)\n");
        assert_eq!(out, "func f() -> void:\n    print(1, 2, 3)\n");
    }

    #[test]
    fn long_call_wraps_with_trailing_comma() {
        let src = "func f() -> void:\n\tconfigure(the_first_long_argument_value, the_second_long_argument_value, the_third_long_argument_value)\n";
        let out = check_roundtrip(src);
        let expected = "\
func f() -> void:
    configure(
        the_first_long_argument_value,
        the_second_long_argument_value,
        the_third_long_argument_value,
    )
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
    if n > 0:
        print(1)
    elif n < 0:
        print(2)
    else:
        print(3)
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
        assert_eq!(out, "func f() -> void:\n    var a := 1\n\n    var b := 2\n");
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
        assert!(out.contains("[\n    aaaaaaaaaaaaaa,\n"), "got:\n{out}");
        assert!(out.ends_with("    ffffffffffffff,\n]\n"), "got:\n{out}");
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
