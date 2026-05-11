use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Yes = 0,
    No  = 1,
}

// ─────────────────────────────────────────────────────────────────────────────
// Market  (PDA: ["market", game_id, market_index])
// ─────────────────────────────────────────────────────────────────────────────

#[account]
pub struct Market {
    pub game_id:             u64,
    pub market_index:        u8,
    pub question:            String,   // 4 + 128
    pub yes_supply:          u64,
    pub no_supply:           u64,
    pub total_volume:        u64,
    pub resolved:            bool,
    pub outcome:             Option<Outcome>,  // 2 bytes
    pub expires_at:          i64,
    pub created_at:          i64,
    pub fee_balance:         u64,
    pub bump:                u8,
    pub vault_bump:          u8,
    pub lp_withdrawn:        bool,
    pub resolved_at:         i64,   // timestamp when market was resolved
    pub winner_payout_ratio: u64,   // scaled by 1e9, set during withdraw_lp
}

impl Market {
    pub const LEN: usize = 8   // discriminator
        + 8                    // game_id
        + 1                    // market_index
        + 4 + 128              // question
        + 8                    // yes_supply
        + 8                    // no_supply
        + 8                    // total_volume
        + 1                    // resolved
        + 2                    // outcome (Option<Outcome>)
        + 8                    // expires_at
        + 8                    // created_at
        + 8                    // fee_balance
        + 1                    // bump
        + 1                    // vault_bump
        + 1                    // lp_withdrawn
        + 8                    // resolved_at
        + 8                    // winner_payout_ratio
        + 32;                  // PADDING: buffer for future upgrades
}
// ─────────────────────────────────────────────────────────────────────────────
// UserPosition  (PDA: ["position", market_key, user_key])
// ─────────────────────────────────────────────────────────────────────────────
// Never auto-closed. Only closeable by the user after claimed == true.

#[account]
pub struct UserPosition {
    pub is_initialized: bool,
    pub user:         Pubkey,    // 32
    pub game_id:      u64,       // 8
    pub market_index: u8,        // 1
    pub yes_shares:   u64,       // 8
    pub no_shares:    u64,       // 8
    /// True after claim_payout or refund_expired. Prevents double-claim.
    pub claimed:      bool,      // 1
    pub bump:         u8,        // 1
}

impl UserPosition {
    // 8 (discriminator) + 1 (bool) + 32 (Pubkey) + 8 (u64) + 1 (u8) + 8 (u64) + 8 (u64) + 1 (bool) + 1 (u8)
    pub const LEN: usize = 8 + 1 + 32 + 8 + 1 + 8 + 8 + 1 + 1;
}
