extends Node


func greet(name:String)->String:
	return "Hello, "+name+"!"


func _ready()->void:
	var msg:=greet("world")
	print(msg)
