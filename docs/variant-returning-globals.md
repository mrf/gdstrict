# Variant-returning `@GlobalScope` globals (Godot 4.6.2 audit)

Background for `gdstrict-ugw`. Downstream (keystone `ks-uw0`) reported that
`gdstrict check` passed `int(round(_breath * 100.0))` while Godot 4.6.2's own parser
rejected the same line at `warnings=2`.

## What actually went wrong

The report's hypothesis was that gdstrict carries a builtin return-type table modelling
`round()` as returning `float`. It does not — gdstrict has no type model of its own. It
drives Godot's analyzer headlessly and recovers each warning's code by matching the
engine's human-readable message against templates in
`crates/gdstrict-strict/src/classify.rs`.

The real defect was one layer down. `unsafe_call_argument` is injected into every check
(`STRICT_WARNINGS` in `crates/gdstrict-strict/src/lib.rs`), so Godot *did* emit the
warning — but `classify_4x` had no template for it, so the diagnostic came back with
`code: None`. `SeverityConfig::apply` can only promote a diagnostic it can key on, so an
uncoded warning falls through to `Action::Warn` regardless of preset, and `check` counts
it as a non-failing warning. Hence exit 0 on code the engine rejects.

Verified by running Godot 4.6.2 directly on the reproduction:

```
$ godot --headless --check-only --path repro --script variant.gd --debug
WARNING: The argument 1 of the constructor "int()" requires the subtype "int", "bool",
         or "float" but the supertype "Variant" was provided.
     at: GDScript::reload (res://variant.gd:5)
```

The fix adds `codes::UNSAFE_CALL_ARGUMENT` and the matching `classify_4x` rule; the
strict preset's existing `UNSAFE_*` family rule then promotes it to an error.

## Message templates

Two shapes share one stem, `requires the subtype … was provided`:

| Call kind | Template |
|---|---|
| Function, builtin global, or builtin method (Godot says "function" for all three) | `The argument N of the function "NAME()" requires the subtype "T" but the supertype "S" was provided.` |
| Builtin constructor | `The argument N of the constructor "NAME()" requires the subtype "T", "U", or "V" but the supertype "S" was provided.` |

The classifier keys on the shared stem rather than the `The argument N of the …` prefix,
so both forms and any subtype alternation land on one rule.

## Audit method

Each candidate global's result was fed to a typed parameter (`func sink(v: float)`) and
the analyzer run headlessly. A `Variant` return produces `unsafe_call_argument`; a
concrete return produces nothing. Godot `4.6.2.stable.official.71f334935`.

## Result — Variant-returning `@GlobalScope` math globals

These 11 return `Variant`. Each has a typed `*f`/`*i` variant precisely because the bare
form does not:

| Variant-returning | Typed variants |
|---|---|
| `abs` | `absf`, `absi` |
| `ceil` | `ceilf`, `ceili` |
| `clamp` | `clampf`, `clampi` |
| `floor` | `floorf`, `floori` |
| `lerp` | `lerpf` |
| `max` | `maxf`, `maxi` |
| `min` | `minf`, `mini` |
| `round` | `roundf`, `roundi` |
| `sign` | `signf`, `signi` |
| `snapped` | `snappedf`, `snappedi` |
| `wrap` | `wrapf`, `wrapi` |

Every typed variant above was confirmed clean.

### Audited and **not** Variant-returning

Checked because they are plausibly in the same family, but concretely typed in 4.6.2 —
notably `remap`, which the docs' `Variant` signature might suggest otherwise:

`remap`, `pingpong`, `posmod`, `fposmod`, `fmod`, `nearest_po2`, `step_decimals`,
`smoothstep`, `inverse_lerp`, `move_toward`, `rotate_toward`, `angle_difference`,
`lerp_angle`, `cubic_interpolate`, `bezier_interpolate`, `ease`, `db_to_linear`,
`linear_to_db`, `sqrt`, `pow`, `log`, `exp`, `sin`, `cos`, `tan`, `atan2`, `deg_to_rad`,
`rad_to_deg`, `randf`, `randi`, `randf_range`, `randi_range`, `randfn`,
`is_equal_approx`.

### Variant-returning, outside the math family

Surfaced by the same audit; they trip the identical diagnostic and the fix covers them
for free: `type_convert`, `str_to_var`, `bytes_to_var`.

## Fixtures

- `fixtures/strict_variant_project/variant_globals.gd` — the failing half: the original
  `int(round(x))` plus one typed-arg feed per Variant-returning global. `check` must exit
  1 with 12 `UNSAFE_CALL_ARGUMENT` errors.
- `fixtures/strict_clean_project/clean.gd` — the passing half: the typed `*f`/`*i`
  mirrors (`roundi`, `absf`, `clampi`, `maxf`) must stay exit 0.

## Scope note

The audit surfaced an adjacent, unfixed gap outside this ticket: Godot 4.6.2 emits
`INFERRED_DECLARATION` as `Variable "s" has an implicitly inferred static type.`, but
`classify_4x` matches only `inferred from a Variant value` — so `var s := ...` degrades to
an uncoded warning the same way. Same class of defect, separate fix.
