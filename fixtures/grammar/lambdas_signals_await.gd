extends Node

signal health_changed(old: int, new: int)
signal died

var health: int = 100

func _ready() -> void:
	health_changed.connect(func(old: int, new: int) -> void:
		print("health: %d -> %d" % [old, new])
	)
	var doubler := func(x: int) -> int: return x * 2
	print(doubler.call(21))

	var ids := [1, 2, 3].map(func(n): return n * n)
	var evens := ids.filter(func(n): return n % 2 == 0)
	print(ids, evens)

	await get_tree().create_timer(1.0).timeout
	await _async_work()
	died.emit()

func _async_work() -> void:
	await get_tree().process_frame
