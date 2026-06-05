extends Node

# Godot 4.4+ typed collections — the syntax that broke gdformat (issue #363).
var tweens: Dictionary[String, Tween] = {}
var scores: Dictionary[StringName, int] = {}
var items: Array[Resource] = []
var grid: Array[Array[int]] = []
var nested: Dictionary[String, Array[Vector2]] = {}

func _ready() -> void:
	var local: Array[Node] = get_children()
	for child: Node in local:
		print(child.name)
	var typed_dict: Dictionary[int, String] = {1: "one", 2: "two"}
	print(typed_dict.size(), tweens, scores, items, grid, nested)
