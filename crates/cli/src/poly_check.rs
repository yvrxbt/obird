//! Polymarket connector smoke test — verifies auth, book feed, and optionally a live order.
//!
//! Usage:
//!   source .env && cargo run --bin trading-cli -- poly-check          # auth + book only
//!   source .env && cargo run --bin trading-cli -- poly-check --live   # + place + cancel a $0.02 test order
//!
//! Steps:
//!   1. Parse PREDICT_PRIVATE_KEY → derive signing address on Polygon
//!   2. Authenticate with POLY_API_KEY / POLY_SECRET / POLY_PASSPHRASE
//!   3. Fetch server time (confirms CLOB REST reachability)
//!   4. Fetch BBO for YES + NO tokens of market 143028
//!   5. List open Polymarket orders for this account
//!   6. [--live only] Place a deep-limit $0.02 test order on the NO token, then immediately cancel it

use std::str::FromStr as _;

use alloy::signers::Signer as _;
use alloy::signers::local::PrivateKeySigner;
use polymarket_client_sdk::{
    POLYGON,
    auth::{Normal, state::Authenticated},
    clob::{Client, Config},
    clob::types::{OrderType, Side},
    clob::types::request::{OrderBookSummaryRequest, OrdersRequest},
    types::U256,
};
use rust_decimal_macros::dec;

/// YES and NO CLOB token IDs for predict.fun market 143028 (Russia-Ukraine ceasefire).
const YES_TOKEN: &str = "8501497159083948713316135768103773293754490207922884688769443031624417212426";
const NO_TOKEN:  &str = "2527312495175492857904889758552137141356236738032676480522356889996545113869";

pub async fn run(live_order_test: bool) -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    // ── Env vars ──────────────────────────────────────────────────────────────

    let private_key = std::env::var("PREDICT_PRIVATE_KEY")
        .map_err(|_| anyhow::anyhow!("PREDICT_PRIVATE_KEY not set"))?;

    // ── Step 1: signing address ───────────────────────────────────────────────

    let signer = PrivateKeySigner::from_str(&private_key)
        .map_err(|e| anyhow::anyhow!("invalid PREDICT_PRIVATE_KEY: {e}"))?
        .with_chain_id(Some(POLYGON));

    println!("[1] Private key OK");
    println!("    Signing address : {}", signer.address());
    println!("    Chain ID        : {} (Polygon mainnet)", POLYGON);
    println!("    USDC needed at this address on Polygon for live hedge orders");
    println!();

    // ── Step 2: authenticate (derive API key from private key) ────────────────
    // The SDK calls create_or_derive_api_key which creates the key if new,
    // or retrieves it deterministically if already existing. No POLY_API_KEY
    // env var needed — the key is derived from PREDICT_PRIVATE_KEY.

    let config = Config::builder().use_server_time(true).build();
    let client: Client<Authenticated<Normal>> = Client::new("https://clob.polymarket.com", config)?
        .authentication_builder(&signer)
        // No .credentials() — let SDK derive from private key
        .authenticate()
        .await
        .map_err(|e| anyhow::anyhow!("authenticate failed: {e}"))?;

    println!("[2] Polymarket CLOB authenticated (key derived from PREDICT_PRIVATE_KEY)");
    println!();

    // ── Step 3: server time ───────────────────────────────────────────────────

    let ts = client.server_time().await
        .map_err(|e| anyhow::anyhow!("server_time() failed: {e}"))?;
    println!("[3] Server time: {ts}");
    println!();

    // ── Step 4: order books ───────────────────────────────────────────────────

    let yes_id = U256::from_str(YES_TOKEN)?;
    let no_id  = U256::from_str(NO_TOKEN)?;

    let yes_req = OrderBookSummaryRequest::builder().token_id(yes_id).build();
    let no_req  = OrderBookSummaryRequest::builder().token_id(no_id).build();

    let yes_book = client.order_book(&yes_req).await
        .map_err(|e| anyhow::anyhow!("order_book(YES) failed: {e}"))?;
    let no_book  = client.order_book(&no_req).await
        .map_err(|e| anyhow::anyhow!("order_book(NO) failed: {e}"))?;

    let yes_bid = yes_book.bids.first().map(|l| l.price.to_string()).unwrap_or("none".into());
    let yes_ask = yes_book.asks.first().map(|l| l.price.to_string()).unwrap_or("none".into());
    let no_bid  = no_book.bids.first().map(|l| l.price.to_string()).unwrap_or("none".into());
    let no_ask  = no_book.asks.first().map(|l| l.price.to_string()).unwrap_or("none".into());

    println!("[4] Market 143028 — Russia-Ukraine Ceasefire before GTA VI");
    println!("    YES  bid={yes_bid}  ask={yes_ask}  (tick={})", yes_book.tick_size);
    println!("    NO   bid={no_bid}  ask={no_ask}  (tick={})", no_book.tick_size);
    println!("    Hedge: predict YES fill → buy NO @ ask={no_ask}");
    println!("    Hedge: predict NO fill  → buy YES @ ask={yes_ask}");
    println!();

    // ── Step 5: open orders ───────────────────────────────────────────────────

    let open = client.orders(&OrdersRequest::default(), None).await
        .map_err(|e| anyhow::anyhow!("orders() failed: {e}"))?;
    println!("[5] Open Polymarket orders: {}", open.data.len());
    for o in &open.data {
        println!("    {} {} @ {} size={}", o.id, o.side, o.price, o.original_size);
    }
    println!();

    // ── Step 6 (--live): place + cancel a deep test order ────────────────────

    if live_order_test {
        println!("[6] LIVE ORDER TEST — placing 5-share NO @ 0.01 (deep below market)");

        let signable = client
            .limit_order()
            .token_id(no_id)
            .order_type(OrderType::GTC)
            .price(dec!(0.01))    // 1 cent — deep below market, will not fill
            .size(dec!(5))        // 5 shares = minimum order size on Polymarket CLOB
            .side(Side::Buy)
            .build()
            .await
            .map_err(|e| anyhow::anyhow!("build order: {e}"))?;

        let signed = client.sign(&signer, signable).await
            .map_err(|e| anyhow::anyhow!("sign: {e}"))?;

        let resp = client.post_order(signed).await
            .map_err(|e| anyhow::anyhow!("post_order: {e}"))?;

        println!("    Placed: order_id={} status={:?} success={}", resp.order_id, resp.status, resp.success);

        if resp.success && !resp.order_id.is_empty() {
            let cancel = client.cancel_order(&resp.order_id).await
                .map_err(|e| anyhow::anyhow!("cancel_order: {e}"))?;
            println!("    Cancelled: {:?}", cancel.canceled);
        }
        println!();
    }

    println!("=== poly-check PASSED ===");
    println!();
    println!("Run the bot:");
    println!("  source .env && RUST_LOG=quoter=info,connector_polymarket=info,connector_predict_fun=info \\");
    println!("    cargo run --release --bin trading-cli -- live --config configs/markets_poly/143028.toml");
    println!();
    println!("Watch for these startup log lines:");
    println!("  INFO  PolymarketExecutionClient ready address=0x...");
    println!("  INFO  Polymarket CLOB WS feed spawned yes_inst=... no_inst=...");
    println!("  INFO  PredictHedgeStrategy initialized id=predict_points_v1_hedge ...");
    println!();
    println!("After first predict fill, expect:");
    println!("  INFO quoter: HEDGE_TRIGGER predict_inst=PredictFun.Binary.143028-Yes ...");
    println!("  INFO quoter: HEDGE_PLAN hedge_qty=... hedge_price=... hedge_notional=...");
    println!("  INFO quoter: POLY_PLACE instrument=Polymarket.Binary.2527312... ...");

    Ok(())
}
