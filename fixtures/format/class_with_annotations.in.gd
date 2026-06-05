@tool
class_name Enemy
extends CharacterBody2D


@export var max_health:int=100
@export var speed:float=150.0
@export_enum("Patrol","Chase","Flee") var behavior:String


var _health:int


func _ready()->void:
	_health=max_health


func take_damage(amount:int)->void:
	_health=max(0,_health-amount)
	if _health==0:
		_die()


func _die()->void:
	queue_free()


func get_health_ratio()->float:
	return float(_health)/float(max_health)
