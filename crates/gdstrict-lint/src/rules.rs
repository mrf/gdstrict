//! The rule catalog. Phase 2 ships one rule to prove the engine end-to-end; the
//! `gdstrict-lint` plan (PLAN.md §2.3) enumerates the rest (naming, dead code,
//! structure). Each rule lives here as a unit struct implementing [`Rule`] and is
//! registered in [`default_rules`].

use crate::{LintContext, Rule, Severity};
use tree_sitter::Node;

/// The default rule set applied by [`crate::lint`].
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(FunctionNameCase)]
}

/// `function-name-case`: function names must be `snake_case`.
///
/// Godot's style guide names functions in snake_case (private functions keep a
/// leading underscore, e.g. `_ready`). This flags `PascalCase` / `camelCase`
/// definitions like `func DoThing():`.
///
/// The CST shape (from `tree-sitter-gdscript`) is:
/// `(function_definition name: (name) parameters: ... body: ...)`, so we read the
/// `name` field and validate its text.
pub struct FunctionNameCase;

impl Rule for FunctionNameCase {
    fn id(&self) -> &'static str {
        "function-name-case"
    }

    fn check(&self, node: Node, ctx: &mut LintContext) {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let text = ctx.node_text(name);
        if !is_snake_case(text) {
            ctx.report(
                name,
                self.id(),
                Severity::Warning,
                format!("function name `{text}` is not snake_case"),
            );
        }
    }
}

/// Snake_case for our purposes: non-empty and composed only of ASCII lowercase
/// letters, digits, and underscores. This accepts the leading-underscore private
/// convention (`_ready`) and rejects any uppercase (`DoThing`, `doThing`).
fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use crate::lint;

    /// Negative case: a PascalCase function is flagged with the right rule id and
    /// location.
    #[test]
    fn flags_pascal_case_function() {
        let src = "func DoThing() -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "function-name-case");
        assert_eq!(diags[0].line, 1);
        // `DoThing` starts after `func ` → column 5.
        assert_eq!(diags[0].column, 5);
        assert!(diags[0].message.contains("DoThing"));
    }

    /// Positive case: idiomatic snake_case names produce no findings, including
    /// the leading-underscore private convention.
    #[test]
    fn accepts_snake_case_functions() {
        let src = "func do_thing() -> void:\n\tpass\n\nfunc _ready() -> void:\n\tpass\n";
        let diags = lint(src);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    /// camelCase is also rejected (any uppercase fails).
    #[test]
    fn flags_camel_case_function() {
        let src = "func doThing() -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("doThing"));
    }

    /// Names with digits and underscores are fine (e.g. `spawn_wave_2`).
    #[test]
    fn accepts_digits_and_underscores() {
        let src = "func spawn_wave_2() -> void:\n\tpass\n";
        assert!(lint(src).is_empty());
    }

    /// Multiple violations in one file are each reported, in source order.
    #[test]
    fn reports_each_violation_in_order() {
        let src = "func Alpha() -> void:\n\tpass\n\nfunc Beta() -> void:\n\tpass\n";
        let diags = lint(src);
        assert_eq!(diags.len(), 2, "got: {diags:?}");
        assert!(diags[0].line < diags[1].line);
        assert!(diags[0].message.contains("Alpha"));
        assert!(diags[1].message.contains("Beta"));
    }
}
