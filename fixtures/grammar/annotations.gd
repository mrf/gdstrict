@icon("res://icon.svg")
@tool
class_name FancyThing
extends Node2D

@export var speed: float = 100.0
@export_range(0, 10, 0.1) var ratio: float = 1.0
@export_enum("Red", "Green", "Blue") var color_name: String
@export_group("Physics")
@export var mass: float = 1.0
@export_flags("Fire", "Water", "Earth") var elements: int
@export_node_path("Camera2D") var cam_path: NodePath

@onready var sprite: Sprite2D = $Sprite2D
@onready var label := %Label as Label

@export_multiline var description: String

@warning_ignore("unused_variable")
func _process(_delta: float) -> void:
	var unused := 5
