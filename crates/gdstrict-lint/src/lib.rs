//! gdstrict-lint: syntactic style rules over the gdstrict-syntax CST.
//!
//! This is the rule engine, not (yet) a rule *catalog*. It provides three pieces
//! that every syntactic check shares:
//!
//! 1. [`Rule`] — a trait each lint implements. A rule inspects one CST node at a
//!    time and reports findings. Rules are stateless and independently testable.
//! 2. The visitor ([`lint`] / [`lint_with`]) — parses source via `gdstrict-syntax`
//!    and walks the CST once in pre-order, offering every node to every rule.
//! 3. [`Diagnostic`] collection — a [`LintContext`] accumulates findings with
//!    stable rule ids, severities, and 1-based line / 0-based column positions.
//!
//! No Godot is involved: this is pure CST analysis (cf. `gdstrict-strict`, which
//! shells out to the engine for *type* diagnostics). The two diagnostic streams
//! are intentionally separate — syntactic style here, semantic typing there.
//!
//! Adding a rule is: implement [`Rule`], add it to [`rules::default_rules`], and
//! write a positive/negative test pair. See [`rules`] for the worked example.

use tree_sitter::Node;

pub mod rules;

/// How loud a finding is. Mirrors the severity split in `gdstrict-strict` so a CLI
/// can render both diagnostic streams uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A single lint finding, located in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable kebab-case rule id (e.g. `"function-name-case"`). Used for config
    /// keys, suppression, and CLI grouping — so it is `&'static str`, not owned.
    pub rule: &'static str,
    pub severity: Severity,
    /// 1-based line of the offending node's start.
    pub line: usize,
    /// 0-based byte column of the offending node's start.
    pub column: usize,
    pub message: String,
}

/// Per-run state handed to each rule: the source text (for node-text extraction)
/// plus the growing diagnostic list. Rules never see the list directly; they
/// [`report`](LintContext::report) into it.
pub struct LintContext<'src> {
    source: &'src str,
    diagnostics: Vec<Diagnostic>,
}

impl<'src> LintContext<'src> {
    fn new(source: &'src str) -> Self {
        Self {
            source,
            diagnostics: Vec::new(),
        }
    }

    /// The source slice a node spans. Empty string if the node's range is somehow
    /// not valid UTF-8 (it always is for our inputs, but rules shouldn't panic).
    pub fn node_text(&self, node: Node) -> &'src str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }

    /// The raw source text being linted. Exposed for rules that need to walk
    /// subtrees outside the `report` flow (e.g. [`rules::UnusedArgument`]).
    pub(crate) fn source(&self) -> &'src str {
        self.source
    }

    /// Record a finding located at `node`'s start position.
    pub fn report(
        &mut self,
        node: Node,
        rule: &'static str,
        severity: Severity,
        message: impl Into<String>,
    ) {
        let pos = node.start_position();
        self.report_at(pos.row + 1, pos.column, rule, severity, message);
    }

    /// Record a finding at an explicit position. Used by rules that operate on
    /// raw source rather than a single CST node (e.g. [`rules::MaxLineLength`],
    /// which works line-by-line and has no node to anchor to).
    pub fn report_at(
        &mut self,
        line: usize,
        column: usize,
        rule: &'static str,
        severity: Severity,
        message: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            rule,
            severity,
            line,
            column,
            message: message.into(),
        });
    }

    fn into_diagnostics(mut self) -> Vec<Diagnostic> {
        // Pre-order visitation already yields near-sorted output, but multiple
        // rules on one node and child-before-sibling ordering can interleave.
        // Sort by position (then rule id) for a deterministic, reader-friendly
        // report regardless of rule registration order.
        self.diagnostics
            .sort_by(|a, b| (a.line, a.column, a.rule).cmp(&(b.line, b.column, b.rule)));
        self.diagnostics
    }
}

/// A syntactic lint rule. Implementors are stateless visitors: [`check`](Rule::check)
/// is called once per CST node in pre-order, and the rule decides whether the node
/// is relevant (typically by matching `node.kind()`).
///
/// Rules report via [`LintContext::report`] rather than returning, so one node can
/// produce several findings and the engine controls ordering.
pub trait Rule {
    /// Stable kebab-case identifier, also used in every [`Diagnostic`] this rule
    /// emits. Must be unique across the registered rule set.
    fn id(&self) -> &'static str;

    /// Inspect one node. Called for every node in the tree, in pre-order.
    fn check(&self, node: Node, ctx: &mut LintContext);
}

/// Lint `source` with the default rule set ([`rules::default_rules`]).
pub fn lint(source: &str) -> Vec<Diagnostic> {
    lint_with(source, &rules::default_rules())
}

/// Lint `source` with an explicit rule set. Parses once via `gdstrict-syntax`,
/// then walks the CST a single time applying all rules to each node.
///
/// Note: a malformed source still parses (tree-sitter recovers with ERROR nodes),
/// so rules run on a best-effort tree. Rules that care about parse integrity can
/// consult `gdstrict_syntax::defects` themselves; most simply skip ERROR nodes by
/// matching on the kinds they expect.
pub fn lint_with(source: &str, rules: &[Box<dyn Rule>]) -> Vec<Diagnostic> {
    let tree = gdstrict_syntax::parse(source);
    let mut ctx = LintContext::new(source);
    visit(tree.root_node(), rules, &mut ctx);
    ctx.into_diagnostics()
}

/// Pre-order walk: offer this node to every rule, then recurse into children.
fn visit(node: Node, rules: &[Box<dyn Rule>], ctx: &mut LintContext) {
    for rule in rules {
        rule.check(node, ctx);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, rules, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway rule proving the framework is rule-agnostic: flags every
    /// `pass` statement. Demonstrates node-kind matching and `report`.
    struct FlagPass;
    impl Rule for FlagPass {
        fn id(&self) -> &'static str {
            "test-flag-pass"
        }
        fn check(&self, node: Node, ctx: &mut LintContext) {
            if node.kind() == "pass_statement" {
                ctx.report(node, self.id(), Severity::Warning, "found a pass");
            }
        }
    }

    fn rule_set() -> Vec<Box<dyn Rule>> {
        vec![Box::new(FlagPass)]
    }

    #[test]
    fn visitor_reaches_nested_nodes() {
        let src = "func a() -> void:\n\tif true:\n\t\tpass\n";
        let diags = lint_with(src, &rule_set());
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].rule, "test-flag-pass");
        // `pass` is on line 3, indented two tabs (0-based column 2).
        assert_eq!(diags[0].line, 3);
        assert_eq!(diags[0].column, 2);
    }

    #[test]
    fn clean_source_yields_no_diagnostics() {
        let src = "func a() -> int:\n\treturn 1\n";
        assert!(lint_with(src, &rule_set()).is_empty());
    }

    #[test]
    fn diagnostics_are_position_sorted() {
        let src = "func a() -> void:\n\tpass\nfunc b() -> void:\n\tpass\n";
        let diags = lint_with(src, &rule_set());
        assert_eq!(diags.len(), 2);
        assert!(diags[0].line < diags[1].line);
    }

    #[test]
    fn node_text_extracts_span() {
        // White-box: build a context and confirm node_text returns the slice.
        let src = "var hello = 1\n";
        let tree = gdstrict_syntax::parse(src);
        let ctx = LintContext::new(src);
        let root = tree.root_node();
        assert_eq!(ctx.node_text(root).trim(), "var hello = 1");
    }
}
