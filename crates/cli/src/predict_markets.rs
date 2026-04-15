//! `trading-cli predict-markets` — discover active boosted predict.fun markets.
//!
//! Fetches open markets, identifies currently boosted ones, resolves Polymarket
//! token IDs (when available), and can auto-write ready-to-run TOML configs.
//!
//! Usage:
//!   source .env && cargo run --bin trading-cli -- predict-markets
//!   source .env && cargo run --bin trading-cli -- predict-markets --all
//!   source .env && cargo run --bin trading-cli -- predict-markets --write-configs
//!   source .env && cargo run --bin trading-cli -- predict-markets --write-configs --output-dir configs/markets

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashSet;

pub async fn run(
    show_all: bool,
    write_configs: bool,
    output_dir: &str,
    fail_on_missing_poly_token: bool,
) -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let api_key =
        std::env::var("PREDICT_API_KEY").map_err(|_| anyhow::anyhow!("PREDICT_API_KEY not set"))?;
    let private_key = std::env::var("PREDICT_PRIVATE_KEY")
        .map_err(|_| anyhow::anyhow!("PREDICT_PRIVATE_KEY not set"))?;

    let sdk = predict_sdk::PredictClient::new(
        56u64,
        &private_key,
        "https://api.predict.fun".to_string(),
        Some(api_key),
    )
    .map_err(|e| anyhow::anyhow!("SDK init: {e}"))?;

    sdk.authenticate_and_store()
        .await
        .map_err(|e| anyhow::anyhow!("auth: {e}"))?;

    if write_configs {
        std::fs::create_dir_all(output_dir)
            .map_err(|e| anyhow::anyhow!("create output dir {output_dir}: {e}"))?;
        println!("[predict-markets] auto-write enabled → {}", output_dir);
    }

    let markets = sdk
        .get_markets_filtered(Some("OPEN"))
        .await
        .map_err(|e| anyhow::anyhow!("get_markets: {e}"))?;

    let now: DateTime<Utc> = Utc::now();

    let mut boosted: Vec<&predict_sdk::PredictMarket> = Vec::new();
    let mut others: Vec<&predict_sdk::PredictMarket> = Vec::new();

    let mut seen_market_ids = HashSet::new();
    for m in &markets {
        if !seen_market_ids.insert(m.id) {
            continue;
        }
        if m.outcomes.len() < 2 {
            continue;
        }
        if is_boost_active(m, now) {
            boosted.push(m);
        } else {
            others.push(m);
        }
    }

    println!("=== CURRENTLY BOOSTED MARKETS ({}) ===", boosted.len());
    if boosted.is_empty() {
        println!("  None active right now. Check back soon — up to 6 markets are boosted at once.");
    }
    let mut missing_poly = Vec::new();

    for m in &boosted {
        if let Some(label) = print_market(
            m,
            &sdk,
            now,
            write_configs,
            output_dir,
            fail_on_missing_poly_token,
        )
        .await?
        {
            missing_poly.push(label);
        }
    }

    if show_all {
        println!("\n=== ALL OTHER OPEN MARKETS ({}) ===", others.len());
        for m in &others {
            if let Some(label) = print_market(
                m,
                &sdk,
                now,
                write_configs,
                output_dir,
                fail_on_missing_poly_token,
            )
            .await?
            {
                missing_poly.push(label);
            }
        }
    } else if !others.is_empty() {
        println!(
            "\n({} non-boosted open markets — run with --all to see them)",
            others.len()
        );
    }

    if fail_on_missing_poly_token && !missing_poly.is_empty() {
        anyhow::bail!(
            "missing polymarket_yes_token_id for {} market(s): {}",
            missing_poly.len(),
            missing_poly.join(", ")
        );
    }

    Ok(())
}

fn is_boost_active(m: &predict_sdk::PredictMarket, now: DateTime<Utc>) -> bool {
    if !m.is_boosted {
        return false;
    }

    let started = m
        .boost_starts_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc) <= now)
        .unwrap_or(true);

    let not_ended = m
        .boost_ends_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|end| end.with_timezone(&Utc) > now)
        .unwrap_or(false);

    started && not_ended
}

async fn print_market(
    m: &predict_sdk::PredictMarket,
    sdk: &predict_sdk::PredictClient,
    now: DateTime<Utc>,
    write_configs: bool,
    output_dir: &str,
    fail_on_missing_poly_token: bool,
) -> anyhow::Result<Option<String>> {
    let yes = &m.outcomes[0];
    let no = &m.outcomes[1];

    let market_id_str = m.id.to_string();
    let (details_result, ob_result) = tokio::join!(
        sdk.get_market_by_id(m.id),
        sdk.get_orderbook(&market_id_str),
    );

    let details = details_result.ok();

    let poly_condition_id = details
        .as_ref()
        .and_then(|d| d.polymarket_condition_ids.first().cloned());

    let poly_token_ids = if let Some(ref cid) = poly_condition_id {
        match connector_polymarket::client::lookup_token_ids(cid).await {
            Ok(ids) => Some(ids),
            Err(e) => {
                tracing::debug!(condition_id = %cid, error = %e, "Gamma API lookup failed — no Polymarket token IDs");
                None
            }
        }
    } else {
        None
    };

    let (best_bid, best_ask, bid_depth, ask_depth) = match &ob_result {
        Ok(ob) => {
            let bb = ob.bids.first().map(|(p, _)| *p);
            let ba = ob.asks.first().map(|(p, _)| *p);
            let bd: Decimal = ob.bids.iter().map(|(_, s)| s).sum();
            let ad: Decimal = ob.asks.iter().map(|(_, s)| s).sum();
            (bb, ba, bd, ad)
        }
        Err(_) => (None, None, Decimal::ZERO, Decimal::ZERO),
    };

    let decimal_precision = details
        .as_ref()
        .and_then(|d| d.decimal_precision)
        .unwrap_or(3);
    let spread_threshold = details.as_ref().and_then(|d| d.spread_threshold);
    let share_threshold = details.as_ref().and_then(|d| d.share_threshold);

    let boost_ends = m.boost_ends_at.as_deref().unwrap_or("?");
    let boost_mins = m
        .boost_ends_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|end| (end.with_timezone(&Utc) - now).num_minutes())
        .unwrap_or(0);

    println!();
    println!("  id={} \"{}\"", m.id, m.title);
    println!(
        "  boosted={} boost_ends={} (~{}m remaining)",
        m.is_boosted, boost_ends, boost_mins
    );
    println!(
        "  isNegRisk={} isYieldBearing={} feeRateBps={} decimalPrecision={}",
        m.is_neg_risk, m.is_yield_bearing, m.fee_rate_bps, decimal_precision
    );
    println!(
        "  book: bid={} ask={} bid_depth={:.0} ask_depth={:.0}",
        best_bid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into()),
        best_ask
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into()),
        bid_depth,
        ask_depth
    );
    println!("  outcomes:");
    println!("    [0] {} → {}", yes.name, yes.on_chain_id);
    println!("    [1] {} → {}", no.name, no.on_chain_id);

    let instr_yes = format!("PredictFun.Binary.{}-{}", m.id, yes.name);
    let instr_no = format!("PredictFun.Binary.{}-{}", m.id, no.name);

    let min_shares = share_threshold.unwrap_or(dec!(100));
    let min_order_size_estimate = if let (Some(bb), Some(ba)) = (best_bid, best_ask) {
        let mid = (bb + ba) / dec!(2);
        let binding_price = mid.max(Decimal::ONE - mid);
        let raw = min_shares * binding_price;
        (raw / dec!(5)).ceil() * dec!(5)
    } else {
        dec!(70)
    };

    // Default if API omits these fields. Keeps config generation fully automatic.
    let spread_threshold_cfg = spread_threshold.unwrap_or(dec!(0.06));
    let min_shares_cfg = share_threshold.unwrap_or(dec!(100));

    let poly_yes_token = poly_token_ids.as_ref().map(|(yes, _)| yes.as_str());
    let poly_no_token = poly_token_ids.as_ref().map(|(_, no)| no.as_str());
    let missing_poly = poly_yes_token.is_none();

    println!();
    println!(
        "  ── configs/markets/{}.toml ─────────────────────────────────",
        m.id
    );
    println!("  [exchanges.params]");
    println!("  market_id        = {}", m.id);
    println!("  yes_outcome_name = {:?}", yes.name);
    println!("  yes_token_id     = {:?}", yes.on_chain_id);
    println!("  no_outcome_name  = {:?}", no.name);
    println!("  no_token_id      = {:?}", no.on_chain_id);
    println!("  is_neg_risk      = {}", m.is_neg_risk);
    println!("  is_yield_bearing = {}", m.is_yield_bearing);
    println!("  fee_rate_bps     = {}", m.fee_rate_bps);
    match poly_yes_token {
        Some(tok) => println!(
            "  polymarket_yes_token_id = {:?}  # Polymarket CLOB YES token → WS FV source",
            tok
        ),
        None => println!(
            "  # polymarket_yes_token_id = ???  # Gamma API lookup failed or market not on Polymarket"
        ),
    }
    match poly_no_token {
        Some(tok) => println!(
            "  # polymarket_no_token_id  = {:?}  # informational only",
            tok
        ),
        None => {}
    }
    if let Some(cid) = &poly_condition_id {
        println!("  # polymarket_condition_id = {:?}  # for reference", cid);
    }
    println!();
    println!("  instruments = [{:?}, {:?}]", instr_yes, instr_no);
    println!();
    println!("  [strategies.params]");
    println!(
        "  order_size_usdt     = \"{min_order_size_estimate}\"  # covers {min_shares:.0}-share min at current mid"
    );
    println!("  spread_threshold_v  = \"{spread_threshold_cfg}\"");
    println!("  min_shares_per_side = \"{min_shares_cfg}\"");
    println!("  # spread_cents tuning: see PREDICT_QUOTING_DESIGN.md (default 0.02 = 44% score, moderate fills)");
    println!("  ────────────────────────────────────────────────────────────");

    if write_configs {
        if fail_on_missing_poly_token && missing_poly {
            println!("  skipped write (missing polymarket_yes_token_id; strict mode enabled)");
            return Ok(Some(format!("{}:{}", m.id, m.title)));
        }

        let metrics_port = metrics_port_for_market(m.id);
        let toml = render_market_toml(
            m,
            &yes.name,
            &yes.on_chain_id,
            &no.name,
            &no.on_chain_id,
            &instr_yes,
            &instr_no,
            min_order_size_estimate,
            spread_threshold_cfg,
            min_shares_cfg,
            poly_yes_token,
            metrics_port,
        );
        let path = format!("{output_dir}/{}.toml", m.id);
        std::fs::write(&path, toml).map_err(|e| anyhow::anyhow!("write {path}: {e}"))?;
        println!("  wrote {}", path);
    }

    if missing_poly {
        Ok(Some(format!("{}:{}", m.id, m.title)))
    } else {
        Ok(None)
    }
}

fn metrics_port_for_market(market_id: u64) -> u16 {
    // Deterministic port assignment to avoid manual edits during farm expansion.
    // 10000..59999 range keeps distance from common local defaults.
    (10_000 + (market_id % 50_000)) as u16
}

fn render_market_toml(
    m: &predict_sdk::PredictMarket,
    yes_name: &str,
    yes_token_id: &str,
    no_name: &str,
    no_token_id: &str,
    instr_yes: &str,
    instr_no: &str,
    order_size_usdt: Decimal,
    spread_threshold_v: Decimal,
    min_shares_per_side: Decimal,
    polymarket_yes_token_id: Option<&str>,
    metrics_port: u16,
) -> String {
    let mut out = String::new();
    out.push_str("# Auto-generated by: trading-cli predict-markets --write-configs\n");
    out.push_str("# Regenerate anytime; this file is intended to be machine-managed.\n\n");
    out.push_str("[engine]\n");
    out.push_str("tick_interval_ms = 200\n\n");

    out.push_str("[[exchanges]]\n");
    out.push_str("name           = \"predict_fun\"\n");
    out.push_str("api_key_env    = \"PREDICT_API_KEY\"\n");
    out.push_str("secret_key_env = \"PREDICT_PRIVATE_KEY\"\n");
    out.push_str("testnet        = false\n\n");

    out.push_str("[exchanges.params]\n");
    out.push_str(&format!("market_id        = {}\n", m.id));
    out.push_str(&format!("yes_outcome_name = {:?}\n", yes_name));
    out.push_str(&format!("yes_token_id     = {:?}\n", yes_token_id));
    out.push_str(&format!("no_outcome_name  = {:?}\n", no_name));
    out.push_str(&format!("no_token_id      = {:?}\n", no_token_id));
    out.push_str(&format!("is_neg_risk      = {}\n", m.is_neg_risk));
    out.push_str(&format!("is_yield_bearing = {}\n", m.is_yield_bearing));
    out.push_str(&format!("fee_rate_bps     = {}\n", m.fee_rate_bps));
    if let Some(tok) = polymarket_yes_token_id {
        out.push_str(&format!("polymarket_yes_token_id = {:?}\n", tok));
    } else {
        out.push_str(
            "# polymarket_yes_token_id = \"\"  # unavailable from Gamma lookup for this market\n",
        );
    }
    out.push('\n');

    out.push_str("[[strategies]]\n");
    out.push_str("name           = \"predict_points_v1\"\n");
    out.push_str("strategy_type  = \"prediction_quoter\"\n");
    out.push_str(&format!(
        "instruments    = [{:?}, {:?}]\n\n",
        instr_yes, instr_no
    ));

    out.push_str("[strategies.params]\n");
    out.push_str("spread_cents         = \"0.02\"  # distance from poly FV per side — see PREDICT_QUOTING_DESIGN.md\n");
    out.push_str(&format!("order_size_usdt      = \"{}\"\n", order_size_usdt));
    out.push_str("max_position_tokens  = \"500.0\"\n");
    out.push_str("drift_cents          = \"0.02\"\n");
    out.push_str("touch_trigger_cents  = \"0.00\"  # defensive requote when quote reaches top-of-book (0 = at touch)\n");
    out.push_str("touch_retreat_cents  = \"0.02\"  # after touch trigger, requote this far behind top-of-book\n");
    out.push_str("min_quote_hold_secs  = 10\n");
    out.push_str("fill_pause_secs      = 5\n");
    out.push_str("fv_stale_secs        = 90       # must be > 60 (WS recv-timeout); see PREDICT_QUOTING_DESIGN.md\n");
    out.push_str(&format!(
        "spread_threshold_v   = \"{}\"\n",
        spread_threshold_v
    ));
    out.push_str(&format!(
        "min_shares_per_side  = \"{}\"\n\n",
        min_shares_per_side
    ));

    out.push_str("[telemetry]\n");
    out.push_str("log_level      = \"info\"\n");
    out.push_str(&format!("metrics_port   = {}\n", metrics_port));
    out.push_str("enable_tracing = false\n");

    out
}
