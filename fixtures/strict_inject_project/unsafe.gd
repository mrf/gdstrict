extends Node

# Deliberately unsafe / untyped code to provoke analyzer WARNINGS (not errors).

func _ready() -> void:
	# UNTYPED_DECLARATION: no type annotation.
	var thing = get_node("Foo")
	# UNSAFE_METHOD_ACCESS: method not present on the static type (Node).
	thing.do_something()
	# UNSAFE_PROPERTY_ACCESS: property not present on the static type.
	var x = thing.some_property
	# UNSAFE_CAST.
	var n = x as int
	# RETURN_VALUE_DISCARDED: ignoring a returned value.
	compute_value()
	# INTEGER_DIVISION.
	var half = 5 / 2
	print(n, half)

func compute_value() -> int:
	return 42
