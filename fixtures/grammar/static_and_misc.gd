@abstract
class_name Shape
extends RefCounted

# Static variables and functions (Godot 4.1+).
static var instances: int = 0
static var _registry: Dictionary[String, int] = {}

const MAX_SIDES := 12
const COLORS: Array[Color] = [Color.RED, Color.GREEN, Color.BLUE]

static func register(name: String) -> void:
	_registry[name] = _registry.get(name, 0) + 1
	instances += 1

@abstract
func area() -> float

func _init() -> void:
	instances += 1

# Backslash line continuation + multiline string.
func _ready() -> void:
	var total := 1 + \
		2 + \
		3
	var text := """
	multi
	line
	"""
	print(total, text)
	# Ternary, type cast, is-check.
	var n := 5 if instances > 0 else 0
	var node := self as Object
	print(n, node is RefCounted)
