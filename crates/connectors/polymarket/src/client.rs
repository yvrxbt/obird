//! Polymarket Gamma API client — REST lookups for market metadata.
//!
//! The Gamma API (`gamma-api.polymarket.com`) is Polymarket's off-chain market
//! metadata service. Given a condition ID (from predict.fun's `polymarketConditionIds`
//! field), it returns the CLOB token IDs needed to subscribe to the CLOB WS feed.
//!
//! ## Token ID convention
//!
//! Each binary market has two outcome tokens:
//!   - Index 0: YES token (the "positive" outcome)
//!   - Index 1: NO token
//!
//! The `clobTokenIds` field in the Gamma response is a JSON-**encoded** string
//! (i.e., JSON within JSON), so it must be double-parsed.

use serde::Deserialize;

const GAMMA_API_BASE: &str = "https://gamma-api.polymarket.com";

/// Minimal Gamma market record — only the fields we use.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaMarket {
    /// JSON-encoded array of two CLOB token IDs: `"[\"yes_token\", \"no_token\"]"`.
    /// Must be parsed twice: once as a JSON string field, once as a JSON array.
    clob_token_ids: String,
}

/// Given a Polymarket condition ID, return `(yes_token_id, no_token_id)`.
///
/// Calls `GET gamma-api.polymarket.com/markets?condition_ids={condition_id}`.
/// YES = index 0, NO = index 1.
///
/// Note: the Gamma API silently ignores unknown query params — if the filter is
/// named incorrectly (`condition_id`, `conditionId`, `conditionIds`) the endpoint
/// returns the full market list and the first entry looks like a valid match.
/// The only working filter name is `condition_ids` (snake_case, plural).
///
/// # Errors
/// Returns an error if:
/// - The HTTP request fails
/// - No market is found for the given condition ID
/// - The `clobTokenIds` field cannot be parsed (missing or malformed)
pub async fn lookup_token_ids(condition_id: &str) -> anyhow::Result<(String, String)> {
    let url = format!("{GAMMA_API_BASE}/markets?condition_ids={condition_id}");

    let markets: Vec<GammaMarket> = reqwest::get(&url)
        .await
        .map_err(|e| anyhow::anyhow!("Gamma API request failed: {e}"))?
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Gamma API response parse failed: {e}"))?;

    // Defensive check: Gamma returns the full market list when the filter name is
    // wrong. A response with more than one market means our filter silently missed.
    if markets.len() > 1 {
        anyhow::bail!(
            "Gamma API returned {} markets for condition_ids={condition_id} — \
             filter was not applied (API contract changed?)",
            markets.len()
        );
    }

    let market = markets.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!("No Polymarket market found for condition_id={condition_id}")
    })?;

    // clobTokenIds is a JSON-encoded string containing a JSON array — double parse.
    let tokens: Vec<String> = serde_json::from_str(&market.clob_token_ids)
        .map_err(|e| anyhow::anyhow!("Failed to parse clobTokenIds JSON: {e}"))?;

    if tokens.len() < 2 {
        anyhow::bail!(
            "Expected 2 token IDs for condition_id={condition_id}, got {}",
            tokens.len()
        );
    }

    Ok((tokens[0].clone(), tokens[1].clone()))
}
