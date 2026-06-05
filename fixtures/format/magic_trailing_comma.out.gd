extends RefCounted

# Input trailing comma => force the collection to stay expanded.
var arr_expanded := [
	1,
	2,
	3,
]
var dict_expanded := {
	"a": 1,
	"b": 2,
}

# No trailing comma => collapse to one line when it fits.
var arr_flat := [1, 2, 3]
var dict_flat := {"a": 1, "b": 2}


func _ready() -> void:
	# Call argument list with a trailing comma stays expanded.
	configure(
		alpha,
		beta,
	)
	# Without one it collapses.
	configure(alpha, beta)
