extends Node

# Regression fixture for gdstrict-ugw.
#
# Godot's @GlobalScope math globals in the `abs`/`round`/`min` family are declared
# to return **Variant**, not float or int — that is precisely why the engine also
# ships typed `*f`/`*i` variants (roundf/roundi, absf/absi, ...). Feeding one of
# those Variant results to a typed parameter is `unsafe_call_argument`, which the
# strict preset promotes to an error.
#
# Every call below must produce exactly one UNSAFE_CALL_ARGUMENT diagnostic. The
# `int(round(x))` line is the original downstream report (keystone ks-uw0): it
# passed gdstrict clean while Godot's own parser rejected it at warnings=2.
#
# The typed-variant mirror lives in ../strict_clean_project/clean.gd, which must
# stay exit-0 — together the two pin both halves of the contract.
func _ready() -> void:
	var f: float = 1.5
	# The downstream report verbatim: global round() returns Variant, so the
	# int() constructor argument is unsafe.
	var breath_pct: int = int(round(f * 100.0))
	# One typed-arg feed per Variant-returning @GlobalScope math global, audited
	# against Godot 4.6.2 (see docs/variant-returning-globals.md).
	sink(abs(f))
	sink(ceil(f))
	sink(clamp(f, f, f))
	sink(floor(f))
	sink(lerp(f, f, f))
	sink(max(f, f))
	sink(min(f, f))
	sink(round(f))
	sink(sign(f))
	sink(snapped(f, 0.1))
	sink(wrap(f, f, f))
	print(breath_pct)


# Typed sink: the parameter type is what makes each Variant argument unsafe.
func sink(value: float) -> void:
	print(value)
