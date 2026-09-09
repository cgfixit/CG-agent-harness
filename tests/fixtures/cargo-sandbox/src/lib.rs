//! Real dependency and doctest compile under Seatbelt.
//! ```
//! assert_eq!(cgah_sandbox_fixture::value(), 1);
//! ```
pub fn value() -> u8 { 1 }
pub fn accepts_serde<T: serde::Serialize>(_: &T) {}
#[cfg(test)]
mod tests {
    include!("../boundary.rs");
    #[test]
    fn sandbox_boundaries() { boundaries(); super::accepts_serde(&1u8); }
}
