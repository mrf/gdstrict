extends Node


func test_it() -> void:
	assert_int(SomeClass.some_call(a, b).size()) \
		.override_failure_message("msg").is_equal(0)


func chain_method() -> void:
	var result = builder() \
		.with_name("x")


func chain_multi() -> void:
	var value = get_tree() \
		.get_root() \
		.get_node("Main") \
		.find_child("Player")


func nested(items: Array) -> void:
	for item in items:
		if item.is_valid():
			item.get_parent() \
				.remove_child(item)


func operator_continuation(a: int, b: int) -> int:
	var total = a \
		+ b
	if a > 0 \
			and b > 0:
		return total
	return 0


func args_continuation() -> void:
	foo(a, \
		b)
	return \
		1
