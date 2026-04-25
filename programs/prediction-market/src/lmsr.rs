use crate::state::Outcome;
use crate::errors::MarketError;
use anchor_lang::prelude::*;

const PRECISION: u128 = 1_000_000;

/// Computes ln(1 + e^(-x)) in fixed-point where x = diff * PRECISION / b
/// Uses a piecewise approximation with linear interpolation.
fn softplus_neg(x_scaled: u128) -> u128 {
    // For x >= 10, the result rounds to 0 at our precision level
    if x_scaled >= 10 * PRECISION {
        return 0;
    }
    
    // Lookup table: ln(1 + e^(-k)) * PRECISION for k = 0..10
    const TABLE: [u128; 11] = [
        693147, // k=0: ln(2)
        313262, // k=1
        126928, // k=2
        48587,  // k=3
        18150,  // k=4
        6715,   // k=5
        2477,   // k=6
        912,    // k=7
        336,    // k=8
        124,    // k=9
        45,     // k=10
    ];
    
    let idx = ((x_scaled / PRECISION) as usize).min(9);
    let frac = x_scaled % PRECISION;
    
    let lo = TABLE[idx];
    let hi = TABLE[idx + 1];
    
    // Linear interpolation: lo - (lo - hi) * (frac / PRECISION)
    let diff = lo.saturating_sub(hi);
    
    // Safe to unwrap: diff <= 693147, frac < 1_000_000. Fits comfortably in u128.
    let offset = diff.checked_mul(frac).unwrap() / PRECISION; 
    
    lo.saturating_sub(offset)
}

fn lmsr_cost(q1: u64, q2: u64, b: u64) -> Result<u64> {
    let max_q = std::cmp::max(q1, q2) as u128;
    let diff = (q1 as u128).abs_diff(q2 as u128);
    
    let x_scaled = diff
        .checked_mul(PRECISION)
        .ok_or(MarketError::Overflow)?
        .checked_div(b as u128)
        .ok_or(MarketError::Overflow)?;
    
    let ln_term = softplus_neg(x_scaled);
    
    let b_ln = (b as u128)
        .checked_mul(ln_term)
        .ok_or(MarketError::Overflow)?
        .checked_div(PRECISION)
        .ok_or(MarketError::Overflow)?;
    
    let total = max_q.checked_add(b_ln).ok_or(MarketError::Overflow)?;
    
    // EXPLANATION: `total` safely fits inside a u64 because:
    // 1. max_q is naturally bounded by u64 limits.
    // 2. The maximum possible value of the `b_ln` term occurs when supplies are equal (x=0).
    //    At x=0, b_ln = b * ln(2). With b = 14,427,000,000, max b_ln ≈ 10,000,000,000,000.
    //    This maximum added liability easily fits within u64::MAX (~18.4 quintillion).
    u64::try_from(total).map_err(|_| error!(MarketError::Overflow))
}

pub fn calc_shares_out(
    yes_supply: u64,
    no_supply: u64,
    b_scaled: u64,
    outcome: Outcome,
    amount_in: u64,
) -> Result<u64> {
    let current_cost = lmsr_cost(yes_supply, no_supply, b_scaled)?;
    let target_cost = current_cost.checked_add(amount_in).ok_or(MarketError::Overflow)?;

    let supply_cap = match outcome {
        Outcome::Yes => u64::MAX.saturating_sub(yes_supply),
        Outcome::No => u64::MAX.saturating_sub(no_supply),
    };
    
    let max_leverage = amount_in.saturating_mul(50_000);
    
    let mut low = 0u64;
    let mut high = std::cmp::min(max_leverage, supply_cap);
    let mut best_shares = 0u64;

    for _ in 0..64 {
        // FIX 1: Exit when search space is exhausted
        if low > high {
            break;
        }
        
        // FIX 2: Prevent underflow when low > high
        let mid = low + (high.saturating_sub(low)) / 2;

        let (new_yes, new_no) = match outcome {
            Outcome::Yes => (yes_supply.checked_add(mid).ok_or(MarketError::Overflow)?, no_supply),
            Outcome::No => (yes_supply, no_supply.checked_add(mid).ok_or(MarketError::Overflow)?),
        };

        let cost = lmsr_cost(new_yes, new_no, b_scaled)?;

        if cost <= target_cost {
            best_shares = mid;
            low = mid.saturating_add(1);
        } else {
            high = mid.saturating_sub(1);
        }
    }

    #[cfg(debug_assertions)]
    {
        let (check_yes, check_no) = match outcome {
            Outcome::Yes => (yes_supply.saturating_add(best_shares), no_supply),
            Outcome::No => (yes_supply, no_supply.saturating_add(best_shares)),
        };
        if let Ok(check_cost) = lmsr_cost(check_yes, check_no, b_scaled) {
            debug_assert!(check_cost <= target_cost);
        }
    }

    Ok(best_shares)
}

pub fn calc_amount_out(
    yes_supply: u64,
    no_supply: u64,
    b_scaled: u64,
    outcome: Outcome,
    shares_in: u64,
) -> Result<u64> {
    let current_cost = lmsr_cost(yes_supply, no_supply, b_scaled)?;
    
    let (new_yes, new_no) = match outcome {
        Outcome::Yes => (yes_supply.checked_sub(shares_in).ok_or(MarketError::Overflow)?, no_supply),
        Outcome::No => (yes_supply, no_supply.checked_sub(shares_in).ok_or(MarketError::Overflow)?),
    };

    let new_cost = lmsr_cost(new_yes, new_no, b_scaled)?;
    current_cost.checked_sub(new_cost).ok_or(MarketError::Overflow.into())
}