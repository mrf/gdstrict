/// Golden fixture tests for the gdstrict formatter.
///
/// For every fixtures/format/*.in.gd file there must be a matching *.out.gd.
/// Two invariants are checked:
///   1. format(in) == out  — the formatter produces the expected output.
///   2. format(out) == out — the output is idempotent (formatting twice is a no-op).
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/gdstrict-format; fixtures live two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/format")
}

fn collect_pairs() -> Vec<(PathBuf, PathBuf)> {
    let dir = fixtures_dir();
    let mut pairs: Vec<(PathBuf, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read fixtures/format dir at {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("read dir entry").path();
            let name = path.file_name()?.to_str()?.to_owned();
            if name.ends_with(".in.gd") {
                let stem = &name[..name.len() - ".in.gd".len()];
                let out = dir.join(format!("{stem}.out.gd"));
                Some((path, out))
            } else {
                None
            }
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

#[test]
fn golden_fixtures() {
    let pairs = collect_pairs();
    assert!(
        !pairs.is_empty(),
        "no *.in.gd fixtures found in fixtures/format/"
    );

    let mut failures: Vec<String> = Vec::new();

    for (in_path, out_path) in &pairs {
        let label = in_path.file_name().unwrap().to_string_lossy();

        let input = std::fs::read_to_string(in_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", in_path.display()));

        let expected = std::fs::read_to_string(out_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", out_path.display()));

        // Invariant 1: format(in) == out
        let actual = gdstrict_format::format(&input);
        if actual != expected {
            failures.push(format!(
                "[{label}] format(in) != out\n--- expected ---\n{expected}\n--- actual ---\n{actual}"
            ));
            continue; // skip idempotency check if output already wrong
        }

        // Invariant 2: format(out) == out  (idempotency)
        let twice = gdstrict_format::format(&actual);
        if twice != actual {
            failures.push(format!(
                "[{label}] idempotency failed: format(out) != out\n--- first pass ---\n{actual}\n--- second pass ---\n{twice}"
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} golden fixture failure(s):\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    eprintln!("golden_fixtures: {} pairs checked", pairs.len());
}
