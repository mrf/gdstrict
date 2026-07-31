extends Node

# The mirror image of strict_project/unsafe.gd: fully typed and statically safe,
# so the GDScript analyzer emits none of the strict-family warnings. `gdstrict
# check` must exit 0 here even with the strict preset forcing the whole family on.
func _ready() -> void:
	# Typed declaration with a statically-typed initializer — no UNTYPED_DECLARATION.
	var node: Node = self
	# Calling a void method that exists on the static type Node — no
	# UNSAFE_METHOD_ACCESS and no RETURN_VALUE_DISCARDED (it returns nothing).
	node.set_process(false)
	# Arithmetic on a typed int — no UNSAFE_CAST, value is used below.
	var doubled: int = compute_value() * 2
	var total: int = doubled + 1
	# Float division, so no INTEGER_DIVISION.
	var half: float = 5.0 / 2.0
	# The typed `*f`/`*i` mirrors of the Variant-returning @GlobalScope math globals.
	# These return concrete float/int, so feeding them to a typed parameter is safe —
	# no UNSAFE_CALL_ARGUMENT. The Variant-returning bare forms (`round`, `abs`, ...)
	# are the failing half, in ../strict_variant_project/variant_globals.gd.
	var rounded: int = roundi(half)
	var magnitude: float = absf(half)
	var bounded: int = clampi(rounded, 0, 10)
	var largest: float = maxf(magnitude, 1.0)
	# Reading a statically-known property and using every value — nothing discarded.
	print(node.name, total, half, bounded, largest)


func compute_value() -> int:
	return 42
