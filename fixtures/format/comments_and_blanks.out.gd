# Top-level comment describing the module.
extends Node

# Constants section.
const MAX_ITEMS := 99
const MIN_ITEMS := 0

# Player state variables.
var score: int = 0
var lives: int = 3

var combo: int = 0

# Called when node enters the tree.
func _ready() -> void:
    # Initialize score display.
    score = 0
    # Initialize lives display.
    lives = 3

func add_score(points: int) -> void:
    # Clamp score to avoid overflow.
    score = min(score + points, 999999)
    combo += 1

func reset() -> void:
    score = 0
    lives = 3

    combo = 0
