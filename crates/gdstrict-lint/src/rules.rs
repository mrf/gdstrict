//! The rule catalog. Naming-convention rules mirroring gdlint's checks, plus
//! the framework demo rule from Phase 2.1. Each rule is a unit struct
//! implementing [`Rule`] and is registered in [`default_rules`].

use crate::{LintContext, Rule, Severity};
use tree_sitter::Node;

/// The default rule set applied by [`crate::lint`].
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(FunctionNameCase),
        Box::new(VariableNameCase),
        Box::new(ParameterNameCase),
        Box::new(ConstantNameCase),
        Box::new(SignalNameCase),
        Box::new(ClassNameCase),
        Box::new(EnumNameCase),
        Box::new(EnumValueCase),
    ]
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// `snake_case`: lowercase letters, ASCII digits, and underscores only.
/// Accepts a leading underscore (private convention: `_ready`, `_on_press`).
fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `PascalCase`: starts with an uppercase letter; only ASCII letters and digits.
/// No underscores — `My_Class` and `_InnerClass` are rejected.
fn is_pascal_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `SCREAMING_SNAKE_CASE`: uppercase letters, ASCII digits, and underscores.
/// Accepts a leading underscore (private constant convention: `_INTERNAL`).
fn is_screaming_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Shared body for the six "simple" rules: match one node kind, read one named
/// field, apply a predicate, and report if it fails. Handles the early-return
/// pattern so each `check` impl becomes a one-liner.
//
// Eight parameters reads high, but each is a distinct, orthogonal knob for the
// six naming rules that share this body; bundling them into a struct would just
// move the noise to every call site without making the contract clearer.
#[allow(clippy::too_many_arguments)]
fn check_named_field(
    node: Node,
    ctx: &mut LintContext,
    rule_id: &'static str,
    node_kind: &str,
    field: &str,
    predicate: fn(&str) -> bool,
    label: &str,
    style: &str,
) {
    if node.kind() != node_kind {
        return;
    }
    let Some(name) = node.child_by_field_name(field) else {
        return;
    };
    let text = ctx.node_text(name);
    if !predicate(text) {
        ctx.report(
            name,
            rule_id,
            Severity::Warning,
            format!("{label} `{text}` is not {style}"),
        );
    }
}

// ─── rules ────────────────────────────────────────────────────────────────────

/// `function-name-case`: function names must be `snake_case`.
///
/// Godot's style guide names functions in snake_case (private functions keep a
/// leading underscore, e.g. `_ready`). This flags `PascalCase` / `camelCase`
/// definitions like `func DoThing():`.
///
/// CST: `(function_definition name: (name) parameters: ... body: ...)`
pub struct FunctionNameCase;

impl Rule for FunctionNameCase {
    fn id(&self) -> &'static str {
        "function-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "function_definition",
            "name",
            is_snake_case,
            "function name",
            "snake_case",
        );
    }
}

/// `variable-name-case`: variable names must be `snake_case`.
///
/// Covers both class-level members (`var speed: float`) and local variables
/// (`var count := 0`). Private variables with a leading underscore (`_hp`) are
/// accepted.
///
/// CST: `(variable_statement name: (name) ...)`
pub struct VariableNameCase;

impl Rule for VariableNameCase {
    fn id(&self) -> &'static str {
        "variable-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "variable_statement",
            "name",
            is_snake_case,
            "variable name",
            "snake_case",
        );
    }
}

/// `parameter-name-case`: function and signal parameter names must be `snake_case`.
///
/// Handles three parameter forms that appear as children of a `parameters` node:
/// - Untyped: `(identifier)` — the node itself is the name.
/// - Typed: `(typed_parameter (identifier) type: ...)` — first named child.
/// - Typed with default: `(typed_default_parameter (identifier) type: ... value: ...)`.
///
/// CST: `(parameters (identifier | typed_parameter | typed_default_parameter) ...)`
pub struct ParameterNameCase;

impl Rule for ParameterNameCase {
    fn id(&self) -> &'static str {
        "parameter-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        match node.kind() {
            "typed_parameter" | "typed_default_parameter" => {
                // First named child is the parameter name identifier.
                let Some(ident) = node.named_child(0) else {
                    return;
                };
                if ident.kind() != "identifier" {
                    return;
                }
                let text = ctx.node_text(ident);
                if !is_snake_case(text) {
                    ctx.report(
                        ident,
                        self.id(),
                        Severity::Warning,
                        format!("parameter name `{text}` is not snake_case"),
                    );
                }
            }
            "identifier" => {
                // An untyped parameter is a bare `identifier` directly inside `parameters`.
                let Some(parent) = node.parent() else {
                    return;
                };
                if parent.kind() != "parameters" {
                    return;
                }
                let text = ctx.node_text(node);
                if !is_snake_case(text) {
                    ctx.report(
                        node,
                        self.id(),
                        Severity::Warning,
                        format!("parameter name `{text}` is not snake_case"),
                    );
                }
            }
            _ => {}
        }
    }
}

/// `constant-name-case`: constant names must be `SCREAMING_SNAKE_CASE`.
///
/// CST: `(const_statement name: (name) ...)`
pub struct ConstantNameCase;

impl Rule for ConstantNameCase {
    fn id(&self) -> &'static str {
        "constant-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "const_statement",
            "name",
            is_screaming_snake_case,
            "constant name",
            "SCREAMING_SNAKE_CASE",
        );
    }
}

/// `signal-name-case`: signal names must be `snake_case`.
///
/// CST: `(signal_statement name: (name) ...)`
pub struct SignalNameCase;

impl Rule for SignalNameCase {
    fn id(&self) -> &'static str {
        "signal-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "signal_statement",
            "name",
            is_snake_case,
            "signal name",
            "snake_case",
        );
    }
}

/// `class-name-case`: class names must be `PascalCase`.
///
/// Covers both the top-level class identity (`class_name MyClass`) and inner
/// class definitions (`class InnerHelper:`). No underscores, must start with
/// an uppercase letter.
///
/// CST: `(class_name_statement name: (name))` and `(class_definition name: (name) ...)`
pub struct ClassNameCase;

impl Rule for ClassNameCase {
    fn id(&self) -> &'static str {
        "class-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "class_name_statement",
            "name",
            is_pascal_case,
            "class name",
            "PascalCase",
        );
        check_named_field(
            node,
            ctx,
            self.id(),
            "class_definition",
            "name",
            is_pascal_case,
            "class name",
            "PascalCase",
        );
    }
}

/// `enum-name-case`: enum type names must be `PascalCase`.
///
/// CST: `(enum_definition name: (name) body: (enumerator_list ...))`
pub struct EnumNameCase;

impl Rule for EnumNameCase {
    fn id(&self) -> &'static str {
        "enum-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "enum_definition",
            "name",
            is_pascal_case,
            "enum name",
            "PascalCase",
        );
    }
}

/// `enum-value-case`: enum member names must be `SCREAMING_SNAKE_CASE`.
///
/// Enum values are effectively scoped constants; the Godot style guide treats
/// them the same as `const` identifiers.
///
/// CST: `(enumerator left: (identifier) ...)` inside `(enumerator_list ...)`
pub struct EnumValueCase;

impl Rule for EnumValueCase {
    fn id(&self) -> &'static str {
        "enum-value-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_named_field(
            node,
            ctx,
            self.id(),
            "enumerator",
            "left",
            is_screaming_snake_case,
            "enum value",
            "SCREAMING_SNAKE_CASE",
        );
    }
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::lint;

    // ── function-name-case ────────────────────────────────────────────────────

    #[test]
    fn flags_pascal_case_function() {
        let src = "func DoThing() -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "function-name-case");
        assert_eq!(diags[0].line, 1);
        assert_eq!(diags[0].column, 5);
        assert!(diags[0].message.contains("DoThing"));
    }

    #[test]
    fn accepts_snake_case_functions() {
        let src = "func do_thing() -> void:\n\tpass\n\nfunc _ready() -> void:\n\tpass\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_camel_case_function() {
        let src = "func doThing() -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("doThing"));
    }

    #[test]
    fn accepts_digits_and_underscores_in_function() {
        let src = "func spawn_wave_2() -> void:\n\tpass\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn reports_multiple_function_violations_in_order() {
        let src = "func Alpha() -> void:\n\tpass\n\nfunc Beta() -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 2, "got: {diags:?}");
        assert!(diags[0].line < diags[1].line);
        assert!(diags[0].message.contains("Alpha"));
        assert!(diags[1].message.contains("Beta"));
    }

    // ── variable-name-case ────────────────────────────────────────────────────

    #[test]
    fn flags_pascal_case_variable() {
        let src = "var PlayerSpeed: float = 1.0\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "variable-name-case");
        assert!(diags[0].message.contains("PlayerSpeed"));
    }

    #[test]
    fn accepts_snake_case_variable() {
        let src = "var player_speed: float = 1.0\nvar _health: int = 100\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_camel_case_local_variable() {
        let src = "func foo() -> void:\n\tvar localCount := 0\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "variable-name-case");
        assert!(diags[0].message.contains("localCount"));
    }

    #[test]
    fn accepts_snake_case_local_variable() {
        let src = "func foo() -> void:\n\tvar local_count := 0\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    // ── parameter-name-case ───────────────────────────────────────────────────

    #[test]
    fn flags_pascal_case_typed_parameter() {
        let src = "func foo(BadParam: int) -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "parameter-name-case");
        assert!(diags[0].message.contains("BadParam"));
    }

    #[test]
    fn accepts_snake_case_typed_parameter() {
        let src = "func foo(good_param: int, _private: String) -> void:\n\tpass\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_pascal_case_untyped_parameter() {
        let src = "func foo(BadParam) -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "parameter-name-case");
        assert!(diags[0].message.contains("BadParam"));
    }

    #[test]
    fn accepts_snake_case_untyped_parameter() {
        let src = "func foo(good_param) -> void:\n\tpass\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_pascal_case_default_parameter() {
        let src = "func foo(BadParam: int = 0) -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "parameter-name-case");
        assert!(diags[0].message.contains("BadParam"));
    }

    // ── constant-name-case ────────────────────────────────────────────────────

    #[test]
    fn flags_lower_case_constant() {
        let src = "const defaultSpeed := 5\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "constant-name-case");
        assert!(diags[0].message.contains("defaultSpeed"));
    }

    #[test]
    fn accepts_screaming_snake_case_constant() {
        let src = "const MAX_HEALTH := 100\nconst _INTERNAL_FLAG := true\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_pascal_case_constant() {
        let src = "const MyConstant := 42\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "constant-name-case");
        assert!(diags[0].message.contains("MyConstant"));
    }

    // ── signal-name-case ──────────────────────────────────────────────────────

    #[test]
    fn flags_pascal_case_signal() {
        let src = "signal HealthChanged(old: int, new: int)\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "signal-name-case");
        assert!(diags[0].message.contains("HealthChanged"));
    }

    #[test]
    fn accepts_snake_case_signal() {
        let src = "signal health_changed(old: int, new: int)\nsignal died\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_camel_case_signal() {
        let src = "signal healthChanged\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "signal-name-case");
        assert!(diags[0].message.contains("healthChanged"));
    }

    // ── class-name-case ───────────────────────────────────────────────────────

    #[test]
    fn flags_snake_case_class_name_statement() {
        let src = "class_name my_class\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "class-name-case");
        assert!(diags[0].message.contains("my_class"));
    }

    #[test]
    fn accepts_pascal_case_class_name_statement() {
        let src = "class_name MyClass\nextends Node\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_snake_case_inner_class() {
        let src = "class inner_helper:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "class-name-case");
        assert!(diags[0].message.contains("inner_helper"));
    }

    #[test]
    fn accepts_pascal_case_inner_class() {
        let src = "class InnerHelper:\n\tpass\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    // ── enum-name-case ────────────────────────────────────────────────────────

    #[test]
    fn flags_snake_case_enum_name() {
        let src = "enum my_direction { NORTH }\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "enum-name-case")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("my_direction"));
    }

    #[test]
    fn accepts_pascal_case_enum_name() {
        let src = "enum Direction { NORTH, SOUTH, EAST, WEST }\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "enum-name-case")
            .collect();
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    // ── enum-value-case ───────────────────────────────────────────────────────

    #[test]
    fn flags_lower_case_enum_value() {
        let src = "enum Color { red, green, blue }\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "enum-value-case")
            .collect();
        assert_eq!(diags.len(), 3, "got: {diags:?}");
        assert!(diags[0].message.contains("red"));
        assert!(diags[1].message.contains("green"));
        assert!(diags[2].message.contains("blue"));
    }

    #[test]
    fn accepts_screaming_snake_case_enum_value() {
        let src = "enum Direction { NORTH, SOUTH, EAST, WEST }\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "enum-value-case")
            .collect();
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    // ── combined / integration ─────────────────────────────────────────────────

    #[test]
    fn clean_idiomatic_file_has_no_findings() {
        let src = concat!(
            "class_name MyActor\n",
            "extends Node\n",
            "\n",
            "signal health_changed(old_hp: int, new_hp: int)\n",
            "signal died\n",
            "\n",
            "const MAX_HEALTH := 100\n",
            "const _REGEN_RATE := 1\n",
            "\n",
            "var current_health: int = MAX_HEALTH\n",
            "var _team_id: int = 0\n",
            "\n",
            "enum State { IDLE, RUNNING, DEAD }\n",
            "\n",
            "func _ready() -> void:\n",
            "\tvar initial := current_health\n",
            "\tprint(initial)\n",
            "\n",
            "func take_damage(amount: int, _source: Node) -> void:\n",
            "\tcurrent_health -= amount\n",
            "\thealth_changed.emit(current_health + amount, current_health)\n",
            "\n",
            "class Hitbox:\n",
            "\tvar size: float = 1.0\n",
        );
        let diags = lint(src);
        assert!(diags.is_empty(), "unexpected findings:\n{diags:#?}");
    }

    #[test]
    fn mixed_violations_all_reported() {
        let src = concat!(
            "signal HealthChanged\n",
            "const defaultSpeed := 5\n",
            "var PlayerName: String\n",
            "enum color { red }\n",
            "func DoThing(BadParam: int) -> void:\n",
            "\tpass\n",
        );
        let diags = lint(src);
        let rules: Vec<&str> = diags.iter().map(|d| d.rule).collect();
        assert!(rules.contains(&"signal-name-case"), "missing signal rule");
        assert!(
            rules.contains(&"constant-name-case"),
            "missing constant rule"
        );
        assert!(
            rules.contains(&"variable-name-case"),
            "missing variable rule"
        );
        assert!(rules.contains(&"enum-name-case"), "missing enum-name rule");
        assert!(
            rules.contains(&"enum-value-case"),
            "missing enum-value rule"
        );
        assert!(
            rules.contains(&"function-name-case"),
            "missing function rule"
        );
        assert!(rules.contains(&"parameter-name-case"), "missing param rule");
    }
}
