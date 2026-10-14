extends RefCounted


# Reads the `Bus` autoload by name. The untyped `extra` is deliberate: it proves the
# warning pass still runs (and still classifies) once the autoload resolves.
func total() -> int:
	var extra = 1
	return Bus.hits + extra
