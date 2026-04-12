//! Backtest report generation — PnL, Sharpe, drawdown analysis.

use trading_core::types::position::Position;
use rust_decimal::Decimal;

#[derive(Debug)]
pub struct BacktestReport {
    pub final_positions: Vec<Position>,
    pub total_pnl: Decimal,
    pub max_drawdown_pct: f64,
    pub sharpe_ratio: f64,
    pub total_trades: usize,
    pub win_rate: f64,
}

impl BacktestReport {
    pub fn new(positions: Vec<Position>) -> Self {
        let total_pnl = positions
            .iter()
            .map(|p| p.unrealized_pnl.inner())
            .sum::<Decimal>();

        Self {
            final_positions: positions,
            total_pnl,
            max_drawdown_pct: 0.0, // TODO: compute from equity curve
            sharpe_ratio: 0.0,     // TODO: compute from return series
            total_trades: 0,       // TODO: count from fills
            win_rate: 0.0,         // TODO: compute from fills
        }
    }

    pub fn print_summary(&self) {
        println!("\n=== Backtest Report ===");
        println!("Total PnL:       {}", self.total_pnl);
        println!("Max Drawdown:    {:.2}%", self.max_drawdown_pct * 100.0);
        println!("Sharpe Ratio:    {:.3}", self.sharpe_ratio);
        println!("Total Trades:    {}", self.total_trades);
        println!("Win Rate:        {:.1}%", self.win_rate * 100.0);
        println!("Positions:");
        for pos in &self.final_positions {
            println!(
                "  {} | size: {} | entry: {} | pnl: {}",
                pos.instrument, pos.size, pos.avg_entry_price, pos.unrealized_pnl
            );
        }
        println!("======================\n");
    }
}
