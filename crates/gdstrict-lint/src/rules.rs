//! The rule catalog. Naming-convention rules mirroring gdlint's checks, plus
//! dead/redundant code rules from Phase 2.3. Each rule is a unit struct
//! implementing [`Rule`] and is registered in [`default_rules`].

use std::cell::RefCell;
use std::collections::HashSet;

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
        Box::new(UnusedArgument),
        Box::new(UnnecessaryPass),
        Box::new(ExpressionNotAssigned),
        Box::new(NoElseReturn),
        Box::new(NoElifReturn),
        Box::new(ComparisonWithItself),
        Box::new(DuplicatedLoad::default()),
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

// ─── dead / redundant code rules ──────────────────────────────────────────────

/// Walk `node` recursively, inserting the text of every `identifier` into `set`.
/// Used by [`UnusedArgument`] to collect all names referenced inside a body.
fn collect_identifiers(node: Node, source: &[u8], set: &mut HashSet<String>) {
    if node.kind() == "identifier" {
        if let Ok(text) = node.utf8_text(source) {
            set.insert(text.to_string());
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, source, set);
    }
}

/// Collect `(name_text, identifier_node)` for every parameter in a `parameters`
/// node. Handles untyped (`identifier`), typed, and typed-with-default forms.
/// Reusable for any rule that needs to enumerate function parameter names.
fn collect_param_idents<'t>(params: Node<'t>, ctx: &LintContext) -> Vec<(String, Node<'t>)> {
    let mut out: Vec<(String, Node<'t>)> = Vec::new();
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                out.push((ctx.node_text(child).to_string(), child));
            }
            "typed_parameter" | "typed_default_parameter" => {
                if let Some(ident) = child.named_child(0) {
                    if ident.kind() == "identifier" {
                        out.push((ctx.node_text(ident).to_string(), ident));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Return `true` if the last named child of `body` is a `return_statement`.
fn body_ends_with_return(body: Node) -> bool {
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .last()
        .is_some_and(|n| n.kind() == "return_statement")
}

/// Shared implementation for [`NoElseReturn`] and [`NoElifReturn`]: when the
/// `if` body ends with a return, flag any `alternative` children whose kind
/// matches `clause_kind` (either `"else_clause"` or `"elif_clause"`).
fn check_redundant_branch_after_return(
    node: Node,
    ctx: &mut LintContext,
    rule_id: &'static str,
    clause_kind: &str,
    message: &str,
) {
    if node.kind() != "if_statement" {
        return;
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    if !body_ends_with_return(body) {
        return;
    }
    let mut cursor = node.walk();
    for alt in node.children_by_field_name("alternative", &mut cursor) {
        if alt.kind() == clause_kind {
            ctx.report(alt, rule_id, Severity::Warning, message);
        }
    }
}

/// `unused-argument`: function arguments that are never referenced in the body.
///
/// Parameters whose name starts with `_` are explicitly marked unused (private
/// convention: `_delta`, `_event`) and are exempt.
///
/// CST: `(function_definition parameters: (parameters ...) body: (body ...))`
pub struct UnusedArgument;

impl Rule for UnusedArgument {
    fn id(&self) -> &'static str {
        "unused-argument"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };

        let param_nodes = collect_param_idents(params, ctx);
        if param_nodes.is_empty() {
            return;
        }

        let mut used: HashSet<String> = HashSet::new();
        collect_identifiers(body, ctx.source().as_bytes(), &mut used);

        for (name, name_node) in &param_nodes {
            if name.starts_with('_') {
                continue;
            }
            if !used.contains(name.as_str()) {
                ctx.report(
                    *name_node,
                    self.id(),
                    Severity::Warning,
                    format!("argument `{name}` is never used"),
                );
            }
        }
    }
}

/// `unnecessary-pass`: a `pass` statement in a body that has other statements.
///
/// `pass` is only required when a body would otherwise be empty. If the body
/// contains any other statement, the `pass` is redundant.
///
/// CST: `(body ... (pass_statement) ...)`
pub struct UnnecessaryPass;

impl Rule for UnnecessaryPass {
    fn id(&self) -> &'static str {
        "unnecessary-pass"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        if node.kind() != "body" {
            return;
        }

        let mut pass_nodes: Vec<Node> = Vec::new();
        let mut non_pass_count: usize = 0;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "pass_statement" {
                pass_nodes.push(child);
            } else {
                non_pass_count += 1;
            }
        }

        if non_pass_count > 0 {
            for pass_node in pass_nodes {
                ctx.report(
                    pass_node,
                    self.id(),
                    Severity::Warning,
                    "unnecessary pass statement",
                );
            }
        }
    }
}

/// `expression-not-assigned`: an expression used as a statement whose result
/// is silently discarded.
///
/// Function calls and `await` expressions are exempt because they are commonly
/// used for their side effects. Everything else (arithmetic, literals, bare
/// identifiers, attribute access) flags as a likely bug.
///
/// CST: `(expression_statement <expr>)`
pub struct ExpressionNotAssigned;

impl Rule for ExpressionNotAssigned {
    fn id(&self) -> &'static str {
        "expression-not-assigned"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        if node.kind() != "expression_statement" {
            return;
        }
        let Some(expr) = node.named_child(0) else {
            return;
        };
        // Side-effecting or mutating forms are fine as statements.
        // A method call on an attribute (`obj.method()`) parses as `attribute`
        // with an `attribute_call` child — detect that specially.
        let should_flag = match expr.kind() {
            "call" | "await_expression" | "assignment" | "augmented_assignment" => false,
            "attribute" => {
                let mut cursor = expr.walk();
                let has_call = expr
                    .named_children(&mut cursor)
                    .any(|c| c.kind() == "attribute_call");
                !has_call
            }
            _ => true,
        };
        if should_flag {
            ctx.report(
                expr,
                self.id(),
                Severity::Warning,
                format!("expression of type `{}` is not assigned", expr.kind()),
            );
        }
    }
}

/// `no-else-return`: an `else` clause that follows an `if` body which always
/// returns. The `else` is structurally redundant — code after the `if` is only
/// reached when the condition is false.
///
/// CST: `(if_statement body: (body ... (return_statement)) alternative: (else_clause ...))`
pub struct NoElseReturn;

impl Rule for NoElseReturn {
    fn id(&self) -> &'static str {
        "no-else-return"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_redundant_branch_after_return(
            node,
            ctx,
            self.id(),
            "else_clause",
            "else clause is unnecessary after a returning if",
        );
    }
}

/// `no-elif-return`: an `elif` clause that follows an `if` body which always
/// returns. The `elif` is structurally redundant — same reasoning as
/// [`NoElseReturn`].
///
/// CST: `(if_statement body: (body ... (return_statement)) alternative: (elif_clause ...))`
pub struct NoElifReturn;

impl Rule for NoElifReturn {
    fn id(&self) -> &'static str {
        "no-elif-return"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        check_redundant_branch_after_return(
            node,
            ctx,
            self.id(),
            "elif_clause",
            "elif clause is unnecessary after a returning if",
        );
    }
}

/// `comparison-with-itself`: a comparison operator whose left and right
/// operands have identical source text (e.g. `x == x`, `len(a) != len(a)`).
/// This is almost always a copy-paste error.
///
/// Checks `==`, `!=`, `<`, `<=`, `>`, `>=`.
///
/// CST: `(binary_operator left: ... op: "==" right: ...)`
pub struct ComparisonWithItself;

impl Rule for ComparisonWithItself {
    fn id(&self) -> &'static str {
        "comparison-with-itself"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        if node.kind() != "binary_operator" {
            return;
        }
        let Some(op) = node.child_by_field_name("op") else {
            return;
        };
        let op_text = ctx.node_text(op);
        match op_text {
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {}
            _ => return,
        }
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let Some(right) = node.child_by_field_name("right") else {
            return;
        };
        let left_text = ctx.node_text(left);
        let right_text = ctx.node_text(right);
        if left_text == right_text {
            ctx.report(
                node,
                self.id(),
                Severity::Warning,
                format!("comparison of `{left_text}` with itself"),
            );
        }
    }
}

/// `duplicated-load`: a `load(...)` or `preload(...)` call whose path argument
/// has already appeared earlier in the same file. The resource object should be
/// stored once in a variable and reused instead.
///
/// CST: two `(call (identifier "load") arguments: (arguments (string "path")))` nodes
/// with identical path strings.
pub struct DuplicatedLoad {
    seen: RefCell<HashSet<String>>,
}

impl Default for DuplicatedLoad {
    fn default() -> Self {
        Self {
            seen: RefCell::new(HashSet::new()),
        }
    }
}

impl Rule for DuplicatedLoad {
    fn id(&self) -> &'static str {
        "duplicated-load"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        if node.kind() != "call" {
            return;
        }
        // The callee is the first named child (an identifier for simple calls).
        let Some(callee) = node.named_child(0) else {
            return;
        };
        if callee.kind() != "identifier" {
            return;
        }
        let func_name = ctx.node_text(callee);
        if func_name != "load" && func_name != "preload" {
            return;
        }
        let Some(args) = node.child_by_field_name("arguments") else {
            return;
        };
        let Some(first_arg) = args.named_child(0) else {
            return;
        };
        if first_arg.kind() != "string" {
            return;
        }
        let path = ctx.node_text(first_arg);
        let key = format!("{func_name}:{path}");
        let mut seen = self.seen.borrow_mut();
        if seen.contains(&key) {
            ctx.report(
                node,
                self.id(),
                Severity::Warning,
                format!("duplicated {func_name} call for {path}"),
            );
        } else {
            seen.insert(key);
        }
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
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "parameter-name-case")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("BadParam"));
    }

    #[test]
    fn accepts_snake_case_typed_parameter() {
        let src = "func foo(good_param: int, _private: String) -> void:\n\tprint(good_param)\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "parameter-name-case")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_pascal_case_untyped_parameter() {
        let src = "func foo(BadParam) -> void:\n\tpass\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "parameter-name-case")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("BadParam"));
    }

    #[test]
    fn accepts_snake_case_untyped_parameter() {
        let src = "func foo(good_param) -> void:\n\tprint(good_param)\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "parameter-name-case")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_pascal_case_default_parameter() {
        let src = "func foo(BadParam: int = 0) -> void:\n\tpass\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "parameter-name-case")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
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

    // ── unused-argument ───────────────────────────────────────────────────────

    #[test]
    fn flags_unused_typed_argument() {
        let src = "func foo(x: int, y: int) -> int:\n\treturn x\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unused-argument")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains('y'));
    }

    #[test]
    fn accepts_used_argument() {
        let src = "func foo(x: int, y: int) -> int:\n\treturn x + y\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unused-argument")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn accepts_underscore_prefixed_argument() {
        let src = "func foo(_unused: int) -> void:\n\tpass\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unused-argument")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_unused_untyped_argument() {
        let src = "func foo(x, y) -> void:\n\tprint(x)\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unused-argument")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains('y'));
    }

    #[test]
    fn accepts_function_with_no_arguments() {
        let src = "func foo() -> void:\n\tpass\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unused-argument")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    // ── unnecessary-pass ─────────────────────────────────────────────────────

    #[test]
    fn flags_pass_in_non_empty_body() {
        let src = "func foo() -> void:\n\tvar x := 1\n\tpass\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unnecessary-pass")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "unnecessary-pass");
    }

    #[test]
    fn accepts_pass_only_body() {
        let src = "func foo() -> void:\n\tpass\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unnecessary-pass")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_pass_before_other_statements() {
        let src = "func foo() -> int:\n\tpass\n\treturn 1\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "unnecessary-pass")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    // ── expression-not-assigned ───────────────────────────────────────────────

    #[test]
    fn flags_arithmetic_expression_not_assigned() {
        let src = "func foo() -> void:\n\t1 + 2\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "expression-not-assigned")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].line, 2);
    }

    #[test]
    fn flags_string_literal_not_assigned() {
        let src = "func foo() -> void:\n\t\"hello\"\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "expression-not-assigned")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn accepts_call_expression_as_statement() {
        let src = "func foo() -> void:\n\tprint(\"hi\")\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "expression-not-assigned")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn accepts_await_expression_as_statement() {
        let src = "func foo() -> void:\n\tawait some_signal\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "expression-not-assigned")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    // ── no-else-return ────────────────────────────────────────────────────────

    #[test]
    fn flags_else_after_returning_if() {
        let src = "func foo(x: int) -> int:\n\tif x > 0:\n\t\treturn 1\n\telse:\n\t\treturn -1\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "no-else-return")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn accepts_else_after_non_returning_if() {
        let src = "func foo(x: int) -> void:\n\tif x > 0:\n\t\tprint(x)\n\telse:\n\t\tprint(0)\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "no-else-return")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn accepts_if_without_else() {
        let src = "func foo(x: int) -> void:\n\tif x > 0:\n\t\treturn\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "no-else-return")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    // ── no-elif-return ────────────────────────────────────────────────────────

    #[test]
    fn flags_elif_after_returning_if() {
        let src =
            "func foo(x: int) -> int:\n\tif x > 0:\n\t\treturn 1\n\telif x == 0:\n\t\treturn 0\n\treturn -1\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "no-elif-return")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn accepts_elif_after_non_returning_if() {
        let src = "func foo(x: int) -> void:\n\tif x > 0:\n\t\tprint(x)\n\telif x == 0:\n\t\tprint(0)\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "no-elif-return")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn accepts_if_without_elif() {
        let src = "func foo(x: int) -> void:\n\tif x > 0:\n\t\treturn\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "no-elif-return")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    // ── comparison-with-itself ────────────────────────────────────────────────

    #[test]
    fn flags_eq_comparison_with_itself() {
        let src = "func foo(x: int) -> bool:\n\treturn x == x\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "comparison-with-itself")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains('x'));
    }

    #[test]
    fn flags_neq_comparison_with_itself() {
        let src = "func foo(x: int) -> bool:\n\treturn x != x\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "comparison-with-itself")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn accepts_comparison_of_different_operands() {
        let src = "func foo(x: int, y: int) -> bool:\n\treturn x == y\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "comparison-with-itself")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_complex_expression_with_itself() {
        let src = "func foo(a: Array) -> bool:\n\treturn a.size() == a.size()\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "comparison-with-itself")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    // ── duplicated-load ───────────────────────────────────────────────────────

    #[test]
    fn flags_duplicate_load() {
        let src =
            "var a = load(\"res://foo.png\")\nvar b = load(\"res://foo.png\")\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "duplicated-load")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("res://foo.png"));
    }

    #[test]
    fn accepts_single_load() {
        let src = "var a = load(\"res://foo.png\")\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "duplicated-load")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn accepts_different_load_paths() {
        let src =
            "var a = load(\"res://foo.png\")\nvar b = load(\"res://bar.png\")\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "duplicated-load")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }

    #[test]
    fn flags_duplicate_preload() {
        let src =
            "var a = preload(\"res://bar.tres\")\nvar b = preload(\"res://bar.tres\")\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "duplicated-load")
            .collect();
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn load_and_preload_same_path_are_not_duplicates() {
        let src =
            "var a = load(\"res://foo.png\")\nvar b = preload(\"res://foo.png\")\n";
        let diags: Vec<_> = lint(src)
            .into_iter()
            .filter(|d| d.rule == "duplicated-load")
            .collect();
        assert!(diags.is_empty(), "got: {diags:#?}");
    }
}
