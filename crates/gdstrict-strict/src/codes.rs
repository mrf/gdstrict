//! Canonical Godot GDScript warning code identifiers.
//!
//! These are the authoritative spellings. All modules within this crate reference
//! these consts rather than duplicating the string literals. The values must match
//! what Godot 4.x emits verbatim — do not change them without updating the
//! classifier templates in `classify.rs` and the preset logic in `config.rs`.
pub const UNTYPED_DECLARATION: &str = "UNTYPED_DECLARATION";
pub const INFERRED_DECLARATION: &str = "INFERRED_DECLARATION";
pub const UNSAFE_METHOD_ACCESS: &str = "UNSAFE_METHOD_ACCESS";
pub const UNSAFE_PROPERTY_ACCESS: &str = "UNSAFE_PROPERTY_ACCESS";
pub const UNSAFE_CAST: &str = "UNSAFE_CAST";
pub const UNSAFE_CALL_ARGUMENT: &str = "UNSAFE_CALL_ARGUMENT";
pub const RETURN_VALUE_DISCARDED: &str = "RETURN_VALUE_DISCARDED";
pub const INTEGER_DIVISION: &str = "INTEGER_DIVISION";
pub const UNUSED_VARIABLE: &str = "UNUSED_VARIABLE";
pub const SHADOWED_VARIABLE: &str = "SHADOWED_VARIABLE";
