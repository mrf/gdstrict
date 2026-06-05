extends Node

enum State { IDLE, RUN, JUMP }

func describe(value: Variant) -> String:
	match value:
		0:
			return "zero"
		1, 2, 3:
			return "small"
		var x when x < 0:
			return "negative %d" % x
		[var a, var b]:
			return "pair %s %s" % [a, b]
		{"type": "player", "hp": var hp}:
			return "player hp=%d" % hp
		_:
			return "other"

func step(state: State) -> void:
	match state:
		State.IDLE:
			pass
		State.RUN, State.JUMP:
			print("moving")
