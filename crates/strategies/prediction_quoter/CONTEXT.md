# strategy-prediction-quoter — LLM Context

## What This Crate Does
Market-making strategy for prediction markets (Polymarket, Predict.fun).
Quotes bid/ask around a fair value received from the fair value service.

## Strategy Logic
1. Receive FairValueUpdate events with P(outcome) and confidence
2. Calculate bid/ask spread based on confidence, position, and params
3. Emit PlaceOrder actions to maintain quotes
4. Adjust quotes on book updates (market moves)
5. Cancel stale quotes on fair value changes

## Key Files
- `quoter.rs` — Implements Strategy trait
- `params.rs` — Strategy parameters (base spread, max position, skew factor)

## Edge Source
Informational — better probability estimate than the market. NOT speed.
The fair value comes from the `fair_value_service` crate via Event::FairValueUpdate.
