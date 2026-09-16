extends Node

# A comment inside a bracketed literal is trivia, not an element: it must never
# receive a separator comma, and its presence forces the collection to stay
# expanded (a `#` would otherwise swallow the rest of a flattened line).
enum Kind {
	## The first kind.
	A,
	## The second kind.
	B,
	# last
}

# Comment as the only content of a collection.
var empty_with_note := [
	# nothing yet
]

# A comment on the opener's line moves below the opener.
var opener_note := [  # why this exists
	1,
]

# Inline comments after an element keep the comma before the comment.
var inline_notes := {
	"a": 1,  # first
	"b": 2,  # second
}


func f(a: int, b: int) -> void:
	var d: Dictionary = {
		"a": 1,
		# a comment
		"b": 2,
	}
	var t: Dictionary[String, int] = {
		"x": 1,
		# typed dict comment
		"y": 2,
		# last item comment
	}
	print(d, t)
	for pair: Array in [
		["a", a],
		# a comment explaining the next entry
		["b", b],
	]:
		print(pair)
	configure(
		# leading comment on the first argument
		alpha,
		beta,  # inline after beta
	)
	# The nested-collection double indent below is a separate (pre-existing)
	# layout quirk, not part of this fixture's contract.
	var p := PackedInt32Array([
		# Top row
		1, 2,
		# Bottom row
		3, 4,
	])
	print(p)


func g(
	# the first parameter
	a: int,
	b: int,  # inline
) -> void:
	print(a, b)
