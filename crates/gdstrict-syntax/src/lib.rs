//! gdstrict-syntax: thin wrapper over tree-sitter-gdscript.
//!
//! For Phase 0 this exists to answer one question: does the grammar parse modern
//! Godot 4.x syntax without ERROR/MISSING nodes? See `grammar_currency` test.

use tree_sitter::{Node, Parser, Tree};

/// A parse defect: a tree-sitter ERROR or MISSING node, with location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDefect {
    pub kind: DefectKind,
    /// 1-based line of the defect start.
    pub line: usize,
    /// 0-based byte column of the defect start.
    pub column: usize,
    /// The node's grammar type name (e.g. "ERROR", or the missing token kind).
    pub node_kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefectKind {
    /// tree-sitter could not parse this region.
    Error,
    /// tree-sitter inserted a node it expected but did not find.
    Missing,
}

/// Build a parser configured for GDScript.
pub fn parser() -> Parser {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_gdscript::LANGUAGE.into())
        .expect("load tree-sitter-gdscript grammar");
    p
}

/// Parse source into a tree-sitter CST.
pub fn parse(source: &str) -> Tree {
    parser()
        .parse(source, None)
        .expect("tree-sitter parse never returns None without a timeout/cancellation")
}

/// Walk the tree and collect every ERROR / MISSING node.
pub fn defects(tree: &Tree) -> Vec<ParseDefect> {
    let mut out = Vec::new();
    collect(tree.root_node(), &mut out);
    out
}

fn collect(node: Node, out: &mut Vec<ParseDefect>) {
    if node.is_error() || node.is_missing() {
        let start = node.start_position();
        out.push(ParseDefect {
            kind: if node.is_missing() {
                DefectKind::Missing
            } else {
                DefectKind::Error
            },
            line: start.row + 1,
            column: start.column,
            node_kind: node.kind().to_string(),
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, out);
    }
}

/// Convenience: true if the source parses with no ERROR/MISSING nodes.
pub fn parses_clean(source: &str) -> bool {
    defects(&parse(source)).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_script_parses_clean() {
        assert!(parses_clean(
            "extends Node\n\nfunc _ready() -> void:\n\tpass\n"
        ));
    }

    #[test]
    fn broken_script_has_defects() {
        // `func` with no body is incomplete.
        let defects = defects(&parse("func foo("));
        assert!(!defects.is_empty(), "expected defects, got none");
    }

    /// Phase 0 Spike .1 — grammar currency.
    ///
    /// Parse every fixture under `fixtures/grammar/` and assert zero defects.
    /// Each fixture exercises a slice of modern Godot 4.x syntax. A failure here
    /// is a documented grammar gap, not a test bug — record it in the spike note.
    #[test]
    fn grammar_currency() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/grammar");
        let mut failures = Vec::new();
        let mut count = 0;
        for entry in std::fs::read_dir(dir).expect("read fixtures/grammar dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("gd") {
                continue;
            }
            count += 1;
            let src = std::fs::read_to_string(&path).unwrap();
            let d = defects(&parse(&src));
            if !d.is_empty() {
                failures.push(format!(
                    "{}: {} defect(s): {:?}",
                    path.file_name().unwrap().to_string_lossy(),
                    d.len(),
                    d
                ));
            }
        }
        assert!(count > 0, "no .gd fixtures found in {dir}");
        assert!(
            failures.is_empty(),
            "grammar gaps found in {count} fixtures:\n{}",
            failures.join("\n")
        );
    }
}
