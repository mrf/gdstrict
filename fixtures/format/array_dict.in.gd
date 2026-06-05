extends RefCounted


var short_arr:=[1,2,3]
var short_dict:={"a":1,"b":2}
var typed_arr:Array[int]=[10,20,30]
var typed_dict:Dictionary[String,int]={"x":100,"y":200}


func build_data()->Dictionary:
	var result:={"name":"player","hp":100,"items":["sword","shield"]}
	return result
