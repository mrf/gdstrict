//! Godot version detection and version-gated warning classification.
//!
//! Godot's CLI prints human warning *messages*, not stable codes like
//! `UNSAFE_METHOD_ACCESS` (proposal #12548, which would expose a real API, is still
//! open). We recover the code by matching the message against a table of templates.
//!
//! Those templates are stable within a Godot release line but are **not** a public
//! contract — a future release can reword a warning. So the table is *version-gated*:
//! [`detect_version`] reads `godot --version`, and [`classifier_for`] selects the
//! [`ClassifierTable`] whose version span contains it. Today there is a single table,
//! the 4.x templates verified empirically against Godot 4.6.2 (Phase 0 spike .2); the
//! machinery exists so that when a template changes we add a new table keyed at the
//! version where it changed and the gate picks it up automatically — no caller change,
//! no silent misclassification on the version that diverged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// A Godot release version: the leading `MAJOR.MINOR.PATCH` of a `godot --version`
/// string. Ordering is the natural `(major, minor, patch)` tuple order, which is what
/// the version gate relies on to pick a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GodotVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GodotVersion {
    /// Construct a version from its components. `const` so it can seed the static
    /// table list below.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse the leading `MAJOR.MINOR.PATCH` of a `godot --version` line such as
    /// `4.6.2.stable.official.71f334935`.
    ///
    /// Trailing build metadata (`.stable.official.<hash>`) is ignored. The patch
    /// component is optional — `4.5.stable` parses as `4.5.0` — and any non-numeric
    /// third segment (the release tag in `4.5.stable`) is treated as a missing patch.
    /// Returns `None` only when the major or minor segment is absent or non-numeric.
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.trim().split('.');
        let major = it.next()?.trim().parse().ok()?;
        let minor = it.next()?.trim().parse().ok()?;
        // Patch may be absent (`4.5.stable`) or a non-numeric tag — both mean 0.
        let patch = it.next().and_then(|p| p.trim().parse().ok()).unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }
}

impl std::fmt::Display for GodotVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A message→code rule table that applies to a span of Godot versions.
///
/// `min_version` is the inclusive lower bound: the table covers every version from
/// `min_version` up to (but not including) the next table's `min_version` in
/// [`TABLES`]. The rule logic lives behind a `fn` pointer so each table is fully
/// typed and self-contained, and so the selection machinery can be exercised with a
/// synthetic table list in tests without inventing fake Godot output.
pub struct ClassifierTable {
    /// Human label for diagnostics and tests, e.g. `"4.x"`.
    label: &'static str,
    /// Inclusive lower bound of the version span this table covers.
    min_version: GodotVersion,
    /// Message → warning code. `None` for messages this table does not map.
    classify: fn(&str) -> Option<&'static str>,
}

impl ClassifierTable {
    /// Map a Godot warning message to its code under this table's templates.
    pub fn classify(&self, msg: &str) -> Option<&'static str> {
        (self.classify)(msg)
    }

    /// The table's label (e.g. `"4.x"`), for diagnostics and tests.
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// The inclusive lower bound of the version span this table covers.
    pub fn min_version(&self) -> GodotVersion {
        self.min_version
    }
}

/// All known classifier tables, **ascending by `min_version`**. Selection
/// ([`select_table`]) picks the newest table whose `min_version <= detected`.
///
/// Only one table is populated today: the 4.x templates verified empirically against
/// Godot 4.6.2 (Phase 0 spike .2). When a future Godot reworks a warning message, add
/// a new entry at the version where it changed — keep this list sorted and the gate
/// routes each detected version to the right table automatically.
static TABLES: &[ClassifierTable] = &[ClassifierTable {
    label: "4.x",
    min_version: GodotVersion::new(4, 0, 0),
    classify: classify_4x,
}];

/// Select the [`ClassifierTable`] for `version`.
///
/// `None` (version undetectable — `godot --version` failed or was unparseable) falls
/// back to the newest table, our best guess for an unknown binary. A version newer
/// than every table also uses the newest; a version older than every table uses the
/// oldest. See [`select_table`] for the mechanism.
pub fn classifier_for(version: Option<GodotVersion>) -> &'static ClassifierTable {
    select_table(TABLES, version)
}

/// Pick the newest table whose `min_version <= version` from an ascending-by-version
/// list. Pulled out from [`classifier_for`] so the selection logic can be unit-tested
/// against a synthetic multi-table list — proving the gate routes boundary versions
/// correctly — without depending on the single production table or on real Godot
/// message strings.
///
/// `tables` must be non-empty and sorted ascending by `min_version`.
fn select_table(
    tables: &'static [ClassifierTable],
    version: Option<GodotVersion>,
) -> &'static ClassifierTable {
    let first = tables.first().expect("at least one classifier table");
    let Some(v) = version else {
        // Unknown version: best-effort with the newest table.
        return tables.last().expect("at least one classifier table");
    };
    // Newest table at or below `v`; if `v` predates every table, use the oldest.
    tables
        .iter()
        .rev()
        .find(|t| t.min_version <= v)
        .unwrap_or(first)
}

/// Message → warning code for the Godot 4.x release line.
///
/// Verified against Godot 4.6.2 (Phase 0 spike .2). Godot prints these as
/// human-readable `WARNING:` messages; we recover the analyzer code by template.
fn classify_4x(msg: &str) -> Option<&'static str> {
    let m = msg;
    if m.contains("has no static type") {
        Some("UNTYPED_DECLARATION")
    } else if m.contains("is not present on the inferred type") && m.contains("method") {
        Some("UNSAFE_METHOD_ACCESS")
    } else if m.contains("is not present on the inferred type") {
        Some("UNSAFE_PROPERTY_ACCESS")
    } else if m.starts_with("Casting") && m.contains("unsafe") {
        Some("UNSAFE_CAST")
    } else if m.contains("returns a value that will be discarded") {
        Some("RETURN_VALUE_DISCARDED")
    } else if m.starts_with("Integer division") {
        Some("INTEGER_DIVISION")
    } else if m.contains("inferred from a Variant value") {
        Some("INFERRED_DECLARATION")
    } else if m.contains("is declared but never used in the block") {
        // Godot 4.6.2: `The local variable "x" is declared but never used in
        // the block.` Distinct from unused_parameter ("is never used in the
        // function").
        Some("UNUSED_VARIABLE")
    } else if m.contains("is shadowing an already-declared") {
        // Godot 4.6.2: `The local variable "x" is shadowing an
        // already-declared variable at line N in the current class.` Also
        // fires for the "local function parameter" variant.
        Some("SHADOWED_VARIABLE")
    } else {
        None
    }
}

/// Process-wide cache mapping a Godot binary path to its detected version, so the
/// worker pool doesn't spawn a `--version` subprocess per check. A binary's version
/// is fixed for the life of the run; `None` (detection failed) is cached too, so we
/// don't retry a broken binary on every file.
///
/// This intentionally mirrors the `override_registry`/`lock_registry` idiom in
/// `lib.rs` (a `OnceLock<Mutex<HashMap>>` accessor + poison-recovering lock). They are
/// kept as separate maps on purpose: this one is write-once memoization, the override
/// registry carries restore-on-last-drop refcount semantics — different lifecycles, so
/// a shared generic would couple two unrelated invariants for no real gain.
fn version_cache() -> &'static Mutex<HashMap<PathBuf, Option<GodotVersion>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<GodotVersion>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the version cache, recovering from a poisoned mutex (the map is plain data;
/// a panic elsewhere must not wedge detection for every later check).
fn lock_version_cache() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Option<GodotVersion>>> {
    version_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Detect the running Godot's version by invoking `godot --version`, cached per
/// binary path. Returns `None` when the binary can't be run or its output doesn't
/// parse — callers fall back to the newest classifier table via [`classifier_for`].
pub fn detect_version(godot: &Path) -> Option<GodotVersion> {
    let key = godot.to_path_buf();
    if let Some(cached) = lock_version_cache().get(&key) {
        return *cached;
    }
    let detected = run_version(godot);
    lock_version_cache().insert(key, detected);
    detected
}

/// Run `godot --version` and parse its stdout. Godot prints the version string (e.g.
/// `4.6.2.stable.official.71f334935`) to stdout; we take the first non-empty line.
fn run_version(godot: &Path) -> Option<GodotVersion> {
    let out = Command::new(godot).arg("--version").output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .and_then(GodotVersion::parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- GodotVersion::parse ---

    #[test]
    fn parses_full_version_string() {
        // The exact form `godot --version` prints on the pinned 4.6.2 release.
        assert_eq!(
            GodotVersion::parse("4.6.2.stable.official.71f334935"),
            Some(GodotVersion::new(4, 6, 2))
        );
    }

    #[test]
    fn parses_patchless_version() {
        // `4.5.stable` → patch defaults to 0; the `stable` tag is not a patch number.
        assert_eq!(
            GodotVersion::parse("4.5.stable.official"),
            Some(GodotVersion::new(4, 5, 0))
        );
        assert_eq!(GodotVersion::parse("4.4"), Some(GodotVersion::new(4, 4, 0)));
    }

    #[test]
    fn parses_real_pinned_release_strings() {
        // Representative `--version` strings across pinned 4.x releases.
        let cases = [
            (
                "4.2.2.stable.official.15073afe3",
                GodotVersion::new(4, 2, 2),
            ),
            ("4.3.stable.official.77dcf97d8", GodotVersion::new(4, 3, 0)),
            (
                "4.4.1.stable.official.49a5bc7b6",
                GodotVersion::new(4, 4, 1),
            ),
            ("4.5.stable.official.fe733bd35", GodotVersion::new(4, 5, 0)),
            (
                "4.6.2.stable.official.71f334935",
                GodotVersion::new(4, 6, 2),
            ),
        ];
        for (s, want) in cases {
            assert_eq!(GodotVersion::parse(s), Some(want), "parsing {s}");
        }
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        assert_eq!(
            GodotVersion::parse("  4.6.2.stable  \n"),
            Some(GodotVersion::new(4, 6, 2))
        );
    }

    #[test]
    fn parse_rejects_non_numeric_or_empty() {
        assert_eq!(GodotVersion::parse(""), None);
        assert_eq!(GodotVersion::parse("garbage"), None);
        assert_eq!(GodotVersion::parse("v4.6"), None);
        // Major present but minor missing/non-numeric → None.
        assert_eq!(GodotVersion::parse("4"), None);
        assert_eq!(GodotVersion::parse("4.x"), None);
    }

    #[test]
    fn version_ordering_is_tuple_order() {
        assert!(GodotVersion::new(4, 4, 1) < GodotVersion::new(4, 5, 0));
        assert!(GodotVersion::new(4, 6, 2) > GodotVersion::new(4, 6, 0));
        assert!(GodotVersion::new(3, 9, 9) < GodotVersion::new(4, 0, 0));
    }

    // --- table selection ---

    #[test]
    fn detected_4x_releases_select_the_4x_table() {
        // Every pinned 4.x release the project tests against must route to "4.x".
        for v in [
            GodotVersion::new(4, 2, 2),
            GodotVersion::new(4, 3, 0),
            GodotVersion::new(4, 4, 1),
            GodotVersion::new(4, 5, 0),
            GodotVersion::new(4, 6, 2),
        ] {
            assert_eq!(classifier_for(Some(v)).label(), "4.x", "version {v}");
        }
    }

    #[test]
    fn unknown_version_falls_back_to_newest_table() {
        // `None` (detection failed) and a far-future version both use the newest table.
        assert_eq!(classifier_for(None).label(), "4.x");
        assert_eq!(
            classifier_for(Some(GodotVersion::new(99, 0, 0))).label(),
            "4.x"
        );
    }

    /// Selection mechanism over a *synthetic* multi-table list — proves the gate
    /// routes boundary versions to the right table without inventing real Godot
    /// message strings for versions we have not verified.
    #[test]
    fn select_table_routes_versions_to_their_span() {
        fn never(_m: &str) -> Option<&'static str> {
            None
        }
        static SYNTH: &[ClassifierTable] = &[
            ClassifierTable {
                label: "old",
                min_version: GodotVersion::new(4, 0, 0),
                classify: never,
            },
            ClassifierTable {
                label: "mid",
                min_version: GodotVersion::new(4, 5, 0),
                classify: never,
            },
            ClassifierTable {
                label: "new",
                min_version: GodotVersion::new(4, 7, 0),
                classify: never,
            },
        ];

        let pick = |v: GodotVersion| select_table(SYNTH, Some(v)).label();
        // Below every table → oldest.
        assert_eq!(
            select_table(SYNTH, Some(GodotVersion::new(3, 9, 0))).label(),
            "old"
        );
        // Exact lower bound is inclusive.
        assert_eq!(pick(GodotVersion::new(4, 0, 0)), "old");
        assert_eq!(pick(GodotVersion::new(4, 4, 9)), "old");
        assert_eq!(pick(GodotVersion::new(4, 5, 0)), "mid");
        assert_eq!(pick(GodotVersion::new(4, 6, 2)), "mid");
        assert_eq!(pick(GodotVersion::new(4, 7, 0)), "new");
        // Above every table → newest.
        assert_eq!(pick(GodotVersion::new(9, 0, 0)), "new");
        // Unknown → newest.
        assert_eq!(select_table(SYNTH, None).label(), "new");
    }

    // --- classify_4x message → code mapping ---

    #[test]
    fn classify_4x_maps_known_messages() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        let cases = [
            (
                r#"Variable "thing" has no static type."#,
                "UNTYPED_DECLARATION",
            ),
            (
                r#"The method "do_something()" is not present on the inferred type "Node"."#,
                "UNSAFE_METHOD_ACCESS",
            ),
            (
                r#"The property "some_property" is not present on the inferred type "Node"."#,
                "UNSAFE_PROPERTY_ACCESS",
            ),
            (r#"Casting "x" to "int" is unsafe."#, "UNSAFE_CAST"),
            (
                r#"The function "compute_value()" returns a value that will be discarded if you don't use it."#,
                "RETURN_VALUE_DISCARDED",
            ),
            (
                "Integer division, decimal part will be discarded.",
                "INTEGER_DIVISION",
            ),
            (
                r#"The variable type is being inferred from a Variant value, so it will be typed as Variant."#,
                "INFERRED_DECLARATION",
            ),
        ];
        for (msg, want) in cases {
            assert_eq!(table.classify(msg), Some(want), "classifying: {msg}");
        }
    }

    #[test]
    fn classify_4x_returns_none_for_unmapped_message() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        assert_eq!(table.classify("Some warning we do not map yet."), None);
        assert_eq!(table.classify(""), None);
    }

    #[test]
    fn classify_4x_maps_unused_variable() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        let msg = r#"The local variable "never_used" is declared but never used in the block. If this is intended, prefix it with an underscore: "_never_used"."#;
        assert_eq!(table.classify(msg), Some("UNUSED_VARIABLE"));
    }

    #[test]
    fn classify_4x_maps_shadowed_variable() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        let msg = r#"The local variable "member_value" is shadowing an already-declared variable at line 3 in the current class."#;
        assert_eq!(table.classify(msg), Some("SHADOWED_VARIABLE"));
    }

    #[test]
    fn classify_4x_maps_shadowed_parameter() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        // shadowed_variable also fires for function parameters
        let msg = r#"The local function parameter "member_value" is shadowing an already-declared variable at line 3 in the current class."#;
        assert_eq!(table.classify(msg), Some("SHADOWED_VARIABLE"));
    }

    #[test]
    fn classify_4x_unused_parameter_does_not_map_to_unused_variable() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        // unused_parameter has a distinct message and must not match UNUSED_VARIABLE
        let msg = r#"The parameter "x" is never used in the function "helper"."#;
        assert_eq!(table.classify(msg), None);
    }

    /// Method-vs-property discrimination: both share the "is not present on the
    /// inferred type" stem, so the method rule (which also requires "method") must win
    /// for method messages and the property rule for the rest.
    #[test]
    fn classify_4x_distinguishes_method_from_property() {
        let table = classifier_for(Some(GodotVersion::new(4, 6, 2)));
        assert_eq!(
            table.classify(r#"The method "f()" is not present on the inferred type "Variant"."#),
            Some("UNSAFE_METHOD_ACCESS")
        );
        assert_eq!(
            table.classify(r#"The property "p" is not present on the inferred type "Variant"."#),
            Some("UNSAFE_PROPERTY_ACCESS")
        );
    }

    // --- live detection (Godot-gated) ---

    /// When a real Godot is on PATH, `detect_version` must return a 4.x version and the
    /// gate must route it to the "4.x" table. Skipped when no Godot is available.
    #[test]
    fn live_detect_version_routes_to_table() {
        let Some(godot) = crate::find_godot() else {
            eprintln!("no godot on PATH; skipping");
            return;
        };
        let Some(v) = detect_version(&godot) else {
            eprintln!("godot present but --version unparseable; skipping");
            return;
        };
        assert_eq!(v.major, 4, "expected a Godot 4.x binary, got {v}");
        assert_eq!(classifier_for(Some(v)).label(), "4.x");
        // Second call must hit the cache and agree.
        assert_eq!(detect_version(&godot), Some(v));
    }
}
