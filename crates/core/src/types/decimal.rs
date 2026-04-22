//! Price and Quantity newtypes over rust_decimal.
//! Using newtypes prevents accidentally mixing prices with quantities.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Price(pub Decimal);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Quantity(pub Decimal);

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Price {
    pub fn new(val: Decimal) -> Self {
        Self(val)
    }
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }
    pub fn inner(&self) -> Decimal {
        self.0
    }
}

impl Quantity {
    pub fn new(val: Decimal) -> Self {
        Self(val)
    }
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }
    pub fn inner(&self) -> Decimal {
        self.0
    }
    pub fn abs(&self) -> Self {
        Self(self.0.abs())
    }
}
