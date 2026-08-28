//! Cyclomatic complexity (McCabe) over the GDScript CST.
//!
//! One engine, two consumers: the [`max-complexity`](crate::rules::MaxComplexity)
//! lint rule gates on the number, and `gdstrict complexity` reports it per
//! function so an external tool that owns coverage can compute CRAP scores
//! (`c² × (1 − cov)³ + c`) by joining line spans against line hits.
//!
//! # Counting model
//!
//! Deliberately statement-level and faithful to ruff's `C901` / PyCQA `mccabe`,
//! so a number here is comparable to a number from ruff or radon for
//! equivalently-shaped code. A function's complexity is `1 + decision points`:
//!
//! | CST node | Δ |
//! |---|---|
//! | `if_statement` | +1 |
//! | `elif_clause` | +1 |
//! | `for_statement` | +1 |
//! | `while_statement` | +1 |
//! | `pattern_section` (each `match` arm, including `_`) | +1 |
//! | `lambda` (nested function) | +1 |
//! | `else_clause` | +0 — the default path is not a decision |
//! | ternary, `and` / `or`, `assert` | +0 — expression-level, outside the model |
//!
//! GDScript has no `try` / `except`, so ruff's handler cases have no analogue.
//!
//! A nested lambda contributes its `+1` *and* its body's decision points to the
//! enclosing function, matching how `mccabe` folds a nested `def` into its
//! parent. Lambdas are never reported as their own [`FunctionComplexity`].

use tree_sitter::Node;

/// The name reported for a top-level `_init`, which the grammar parses as a
/// `constructor_definition` with no `name` field.
const CONSTRUCTOR_NAME: &str = "_init";

/// One function's complexity plus the source span a coverage tool joins on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionComplexity {
    /// Function name, qualified by enclosing inner classes (`Inner.method`).
    pub name: String,
    /// 1-based line of the `func` keyword.
    pub line: usize,
    /// 0-based column of the `func` keyword (matches [`crate::Diagnostic`]).
    pub column: usize,
    /// 1-based line of the function's last line (inclusive).
    pub end_line: usize,
    /// McCabe cyclomatic complexity; never less than 1.
    pub complexity: usize,
}

/// Every function in `source`, in source order.
///
/// Reported units are `function_definition` and `constructor_definition` nodes,
/// wherever they appear — top level or inside an inner `class` body.
#[must_use]
pub fn functions(source: &str) -> Vec<FunctionComplexity> {
    let tree = gdstrict_syntax::parse(source);
    let mut out = Vec::new();
    collect(tree.root_node(), source, &mut Vec::new(), &mut out);
    out
}

/// The McCabe complexity of one `function_definition` / `constructor_definition`
/// node. Returns 1 for a function whose body did not parse.
#[must_use]
pub fn of_function(func: Node) -> usize {
    match func.child_by_field_name("body") {
        Some(body) => 1 + decision_points(body),
        None => 1,
    }
}

/// The declared name of a function node. `constructor_definition` has no `name`
/// field in the grammar, so it reports as `_init` — what the user wrote.
#[must_use]
pub fn name_of<'src>(func: Node, source: &'src str) -> &'src str {
    func.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .unwrap_or(CONSTRUCTOR_NAME)
}

/// True for the two node kinds [`functions`] reports and [`of_function`] accepts.
#[must_use]
pub fn is_function(node: Node) -> bool {
    matches!(
        node.kind(),
        "function_definition" | "constructor_definition"
    )
}

/// Walk the tree collecting functions, tracking the enclosing inner-class path so
/// `Inner.method` disambiguates same-named methods in one file.
fn collect(
    node: Node,
    source: &str,
    class_path: &mut Vec<String>,
    out: &mut Vec<FunctionComplexity>,
) {
    if is_function(node) {
        let name = name_of(node, source);
        let start = node.start_position();
        out.push(FunctionComplexity {
            name: qualify(class_path, name),
            line: start.row + 1,
            column: start.column,
            end_line: node.end_position().row + 1,
            complexity: of_function(node),
        });
        // Don't descend: lambdas fold into this function's own number, and
        // GDScript has no nested `func` definitions.
        return;
    }

    let pushed = if node.kind() == "class_definition" {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .unwrap_or("");
        class_path.push(name.to_string());
        true
    } else {
        false
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, class_path, out);
    }

    if pushed {
        class_path.pop();
    }
}

/// `["A", "B"] + "m"` ⇒ `"A.B.m"`; an empty path leaves the name bare.
fn qualify(class_path: &[String], name: &str) -> String {
    if class_path.is_empty() {
        return name.to_string();
    }
    let mut qualified = class_path.join(".");
    qualified.push('.');
    qualified.push_str(name);
    qualified
}

/// Sum the decision points in `node` and all of its descendants.
fn decision_points(node: Node) -> usize {
    let mut total = usize::from(is_decision_point(node.kind()));
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        total += decision_points(child);
    }
    total
}

/// The `+1` set. `else_clause` is deliberately absent: it is the default path,
/// not a branch, which is why `mccabe` and ruff both skip it.
fn is_decision_point(kind: &str) -> bool {
    matches!(
        kind,
        "if_statement"
            | "elif_clause"
            | "for_statement"
            | "while_statement"
            | "pattern_section"
            | "lambda"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Complexity of the single function in `src`.
    fn one(src: &str) -> usize {
        let funcs = functions(src);
        assert_eq!(funcs.len(), 1, "expected exactly one function: {funcs:?}");
        funcs[0].complexity
    }

    #[test]
    fn straight_line_function_is_one() {
        assert_eq!(one("func f() -> void:\n\tpass\n"), 1);
    }

    #[test]
    fn if_adds_one() {
        assert_eq!(one("func f(x: int) -> void:\n\tif x:\n\t\tpass\n"), 2);
    }

    #[test]
    fn elif_adds_one_and_else_adds_none() {
        // if + elif = 3; the else clause is the default path and contributes 0.
        let src = "func f(x: int) -> void:\n\tif x == 1:\n\t\tpass\n\telif x == 2:\n\t\tpass\n\telse:\n\t\tpass\n";
        assert_eq!(one(src), 3);
    }

    #[test]
    fn else_alone_adds_none() {
        let src = "func f(x: int) -> void:\n\tif x:\n\t\tpass\n\telse:\n\t\tpass\n";
        assert_eq!(one(src), 2);
    }

    #[test]
    fn for_adds_one() {
        assert_eq!(
            one("func f() -> void:\n\tfor i in range(3):\n\t\tpass\n"),
            2
        );
    }

    #[test]
    fn while_adds_one() {
        assert_eq!(
            one("func f(x: int) -> void:\n\twhile x > 0:\n\t\tx -= 1\n"),
            2
        );
    }

    #[test]
    fn each_match_arm_adds_one() {
        let src = "func f(x: int) -> void:\n\tmatch x:\n\t\t1:\n\t\t\tpass\n\t\t2:\n\t\t\tpass\n\t\t_:\n\t\t\tpass\n";
        // Three arms, including the `_` wildcard.
        assert_eq!(one(src), 4);
    }

    #[test]
    fn nested_lambda_adds_one_plus_its_own_branches() {
        // lambda (+1) and the `if` inside it (+1) both land on the enclosing func.
        let src = "func f() -> void:\n\tvar g := func(y: int) -> void:\n\t\tif y:\n\t\t\tpass\n";
        assert_eq!(one(src), 3);
    }

    #[test]
    fn ternary_and_boolean_operators_add_nothing() {
        // Statement-level model: expression branching is outside it, as in ruff.
        let src = "func f(x: int) -> int:\n\tvar y := x if x > 0 else -x\n\tif x > 0 and x < 5 or x == 9:\n\t\tpass\n\treturn y\n";
        assert_eq!(one(src), 2);
    }

    #[test]
    fn assert_adds_nothing() {
        assert_eq!(one("func f(x: int) -> void:\n\tassert(x != 0)\n"), 1);
    }

    #[test]
    fn nesting_accumulates() {
        let src = "func f(x: int) -> void:\n\tfor i in range(3):\n\t\tif x:\n\t\t\twhile x > 0:\n\t\t\t\tx -= 1\n";
        assert_eq!(one(src), 4);
    }

    #[test]
    fn top_level_init_reports_as_init() {
        // A top-level `_init` parses as `constructor_definition`, which has no
        // `name` field — the reported name has to be reconstructed.
        let funcs = functions("func _init() -> void:\n\tpass\n");
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "_init");
        assert_eq!(funcs[0].complexity, 1);
    }

    #[test]
    fn inner_class_methods_are_qualified() {
        let src = "class Inner:\n\tfunc m(a: int) -> void:\n\t\tif a:\n\t\t\tpass\n\nfunc m() -> void:\n\tpass\n";
        let funcs = functions(src);
        let names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["Inner.m", "m"]);
        assert_eq!(funcs[0].complexity, 2);
        assert_eq!(funcs[1].complexity, 1);
    }

    #[test]
    fn nested_inner_classes_qualify_with_full_path() {
        let src = "class A:\n\tclass B:\n\t\tfunc m() -> void:\n\t\t\tpass\n";
        let funcs = functions(src);
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "A.B.m");
    }

    #[test]
    fn functions_are_reported_in_source_order_with_spans() {
        let src = "func a() -> void:\n\tpass\n\nfunc b(x: int) -> void:\n\tif x:\n\t\tpass\n";
        let funcs = functions(src);
        assert_eq!(funcs.len(), 2);

        assert_eq!(funcs[0].name, "a");
        assert_eq!(funcs[0].line, 1);
        assert_eq!(funcs[0].column, 0);
        assert_eq!(funcs[0].end_line, 2);

        assert_eq!(funcs[1].name, "b");
        assert_eq!(funcs[1].line, 4);
        assert_eq!(funcs[1].end_line, 6);
        assert_eq!(funcs[1].complexity, 2);
    }

    #[test]
    fn static_function_is_reported() {
        let funcs = functions("static func s() -> void:\n\tpass\n");
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "s");
    }

    #[test]
    fn file_with_no_functions_reports_nothing() {
        assert!(functions("extends Node\n\nvar x := 1\n").is_empty());
    }
}
