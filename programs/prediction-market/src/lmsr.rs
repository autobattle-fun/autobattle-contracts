use anchor_lang::prelude::*;
use crate::state::Outcome;
use crate::errors::MarketError;

/// Scale factor: 1_000_000 = 1 $AUTO (6 decimals).
const SCALE: f64 = 1_000_000.0;

/// Compute shares received for `amount_in` $AUTO tokens spent on `outcome`.
pub fn calc_shares_out(
    yes_supply: u64,
    no_supply: u64,
    b_scaled: u64,
    outcome: Outcome,
    amount_in: u64,
) -> Result<u64> {
    if amount_in == 0 {
        return Ok(0);
    }

    let b = b_scaled as f64 / SCALE;
    let a = amount_in as f64 / SCALE;
    let y = yes_supply as f64 / SCALE;
    let n = no_supply as f64 / SCALE;

    // Delta normalizes the exponents to prevent f64::INFINITY overflows
    let delta = match outcome {
        Outcome::Yes => (n - y) / b,
        Outcome::No  => (y - n) / b,
    };

    let e_a_b = (a / b).exp();
    let e_delta = delta.exp();

    // x = b * ln( e^(a/b) * (1 + e^delta) - e^delta )
    let inner = e_a_b * (1.0 + e_delta) - e_delta;
    
    // Safety check against precision loss drops
    if inner <= 0.0 {
        return err!(MarketError::Overflow);
    }

    let shares_out = b * inner.ln();
    
    // Scale back to u64
    let shares_scaled = (shares_out * SCALE).floor() as u64;
    Ok(shares_scaled)
}

/// Compute $AUTO received for selling `shares_in` of `outcome`.
pub fn calc_amount_out(
    yes_supply: u64,
    no_supply: u64,
    b_scaled: u64,
    outcome: Outcome,
    shares_in: u64,
) -> Result<u64> {
    if shares_in == 0 {
        return Ok(0);
    }

    let b = b_scaled as f64 / SCALE;
    let s = shares_in as f64 / SCALE;
    let y = yes_supply as f64 / SCALE;
    let n = no_supply as f64 / SCALE;

    let delta = match outcome {
        Outcome::Yes => (n - y) / b,
        Outcome::No  => (y - n) / b,
    };

    let e_neg_s_b = (-s / b).exp();
    let e_delta = delta.exp();

    // a = b * ln( (1 + e^delta) / (e^(-s/b) + e^delta) )
    let numerator = 1.0 + e_delta;
    let denominator = e_neg_s_b + e_delta;
    
    if denominator <= 0.0 {
        return err!(MarketError::Overflow);
    }

    let amount_out = b * (numerator / denominator).ln();

    let amount_scaled = (amount_out * SCALE).floor() as u64;
    Ok(amount_scaled)
}

/// Current implied probability of YES (scaled to SCALE = 1_000_000).
pub fn yes_price(yes_supply: u64, no_supply: u64, b_scaled: u64) -> u64 {
    let b = b_scaled as f64 / SCALE;
    let y = yes_supply as f64 / SCALE;
    let n = no_supply as f64 / SCALE;

    let delta = (n - y) / b;
    let e_delta = delta.exp();

    // p_yes = 1 / (1 + e^delta)
    let price = 1.0 / (1.0 + e_delta);
    
    (price * SCALE).floor() as u64
}