extends Node


func classify(n:int)->String:
	if n>0:
		return "positive"
	elif n<0:
		return "negative"
	else:
		return "zero"


func sum_to(limit:int)->int:
	var total:=0
	for i in range(limit):
		if i%2==0:
			total+=i
	return total


func find_first(arr:Array,target:Variant)->int:
	var i:=0
	while i<arr.size():
		if arr[i]==target:
			return i
		i+=1
	return -1
