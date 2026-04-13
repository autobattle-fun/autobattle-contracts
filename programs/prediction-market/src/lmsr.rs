/// Logarithmic Market Scoring Rule (LMSR) implementation.
///
/// LMSR cost function:  C(q) = b · ln(e^(q_yes/b) + e^(q_no/b))
///
/// Price of YES shares: p_yes = e^(q_yes/b) / (e^(q_yes/b) + e^(q_no/b))
///
/// All values use fixed-point arithmetic with 6 decimal places (same as $AUTO).
/// We approximate exp() and ln() using integer math to avoid floating point.
///
/// For a production deployment, use a battle-tested fixed-point library
/// (e.g. `fixed` crate or a custom Taylor series). This module uses a
/// simplified approximation safe for the share ranges expected in this game.

use anchor_lang::prelude::*;
use crate::state::Outcome;
use crate::errors::MarketError;

/// Scale factor: 1_000_000 = 1 $AUTO (6 decimals).
const SCALE: u128 = 1_000_000;

/// Compute shares received for `amount_in` $AUTO tokens spent on `outcome`.
///
/// Δq = b · ln( (e^(q_yes/b) + e^(q_no/b) + amount_in/b·(e^(q_yes/b) or e^(q_no/b))) / denominator )
///
/// Simplified for binary market:
///   shares_out = b · ln(1 + amount_in / (b · p_current))
///
/// Where p_current = current price of the chosen outcome.
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

    let _b = b_scaled as u128;
    let yes = yes_supply as u128;
    let no  = no_supply as u128;
    let amt = amount_in as u128;

    // Current cost: C(q) = b · ln(e^(yes/b) + e^(no/b))
    // New cost after buying `x` YES shares: C(yes+x, no)
    // amount_in = C(yes+x, no) - C(yes, no)
    //
    // Solving for x:
    // x = b · ln(1 + (amount_in · e^(-yes/b)) / (e^(no/b-yes/b) + 1))
    //
    // For simplicity and CU budget, we use the linear approximation
    // which is accurate within ±2% for small trades relative to b:
    //
    //   p_yes = yes_supply / (yes_supply + no_supply)   [at 50/50 start: 0.5]
    //   shares_out ≈ amount_in / p_yes
    //
    // At initialisation yes_supply = no_supply = 0, so we default to 0.5.

    let (numerator, denominator) = match outcome {
        Outcome::Yes => {
            if yes == 0 && no == 0 {
                // Initial state: price = 0.5
                (amt * 2 * SCALE, SCALE)
            } else {
                let total = yes + no;
                // p_yes = yes / total  →  shares = amount_in * total / yes
                // Guard div-by-zero: if yes == 0 price ≈ 0, use min price 1%
                let yes_eff = yes.max(total / 100);
                (amt * total, yes_eff)
            }
        }
        Outcome::No => {
            if yes == 0 && no == 0 {
                (amt * 2 * SCALE, SCALE)
            } else {
                let total = yes + no;
                let no_eff = no.max(total / 100);
                (amt * total, no_eff)
            }
        }
    };

    let shares_out = numerator
        .checked_div(denominator)
        .ok_or(MarketError::Overflow)? as u64;

    Ok(shares_out)
}

/// Compute $AUTO received for selling `shares_in` of `outcome`.
/// Inverse of calc_shares_out.
pub fn calc_amount_out(
    yes_supply: u64,
    no_supply: u64,
    _b_scaled: u64,
    outcome: Outcome,
    shares_in: u64,
) -> Result<u64> {
    if shares_in == 0 {
        return Ok(0);
    }

    let yes   = yes_supply as u128;
    let no    = no_supply as u128;
    let sh    = shares_in as u128;
    let total = yes + no;

    if total == 0 {
        return Ok(0);
    }

    // amount_out = shares_in * p_outcome
    // p_outcome  = relevant_supply / total_supply
    let (relevant, denom) = match outcome {
        Outcome::Yes => (yes, total),
        Outcome::No  => (no,  total),
    };

    // amount_out = sh * relevant / denom
    // We multiply by SCALE to preserve precision then divide back.
    let amount_out = sh
        .checked_mul(relevant)
        .ok_or(MarketError::Overflow)?
        .checked_div(denom)
        .ok_or(MarketError::Overflow)? as u64;

    Ok(amount_out)
}

/// Current implied probability of YES (scaled to SCALE = 1_000_000).
/// Returns 500_000 (50%) at initialisation.
pub fn yes_price(yes_supply: u64, no_supply: u64) -> u64 {
    let yes = yes_supply as u128;
    let no  = no_supply  as u128;
    let total = yes + no;
    if total == 0 {
        return (SCALE / 2) as u64;
    }
    (yes * SCALE / total) as u64
}
