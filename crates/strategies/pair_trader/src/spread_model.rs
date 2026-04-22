//! Spread z-score calculation and half-life estimation.
//! Uses a rolling window of spread observations.

pub struct SpreadModel {
    observations: Vec<f64>,
    lookback: usize,
}

impl SpreadModel {
    pub fn new(lookback: usize) -> Self {
        Self {
            observations: Vec::with_capacity(lookback),
            lookback,
        }
    }

    pub fn update(&mut self, spread: f64) {
        self.observations.push(spread);
        if self.observations.len() > self.lookback {
            self.observations.remove(0);
        }
    }

    pub fn zscore(&self) -> Option<f64> {
        if self.observations.len() < 2 {
            return None;
        }
        let mean = self.observations.iter().sum::<f64>() / self.observations.len() as f64;
        let variance = self
            .observations
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>()
            / (self.observations.len() - 1) as f64;
        let std = variance.sqrt();
        if std < 1e-10 {
            return None;
        }
        let latest = *self.observations.last()?;
        Some((latest - mean) / std)
    }
}
