extends Node

enum Direction {NORTH, SOUTH, EAST, WEST}

func direction_to_vector(dir: Direction) -> Vector2:
    match dir:
        Direction.NORTH:
            return Vector2.UP
        Direction.SOUTH:
            return Vector2.DOWN
        Direction.EAST:
            return Vector2.RIGHT
        Direction.WEST:
            return Vector2.LEFT
        _:
            return Vector2.ZERO

func describe_value(v: Variant) -> String:
    match v:
        0:
            return "zero"
        1, 2, 3:
            return "small positive"
        var x when x < 0:
            return "negative"
        _:
            return "other"
