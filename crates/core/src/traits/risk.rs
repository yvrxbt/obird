//! Pre-trade risk check trait.
//! Called synchronously by the OrderRouter on every action.
//! Must be fast — microseconds, not milliseconds.

use crate::Action;
use crate::error::RiskRejection;
use crate::types::position::Position;

pub trait RiskCheck: Send + Sync {
    /// Check whether an action is allowed given current positions.
    /// Returns Ok(()) if allowed, Err with rejection reason if not.
    fn check(&self, action: &Action, positions: &[Position]) -> Result<(), RiskRejection>;
}
