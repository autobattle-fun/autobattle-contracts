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
    pub game_id:          u64,          // 8
    pub market_index:     u8,           // 1  (0-3 = win markets, 4+ = live)
    pub question:         String,       // 4 + 128
    pub yes_supply:       u64,          // 8  — total YES shares outstanding
    pub no_supply:        u64,          // 8  — total NO shares outstanding
    pub total_volume:     u64,          // 8  — cumulative $AUTO traded
    pub resolved:         bool,         // 1
    pub outcome:          Option<Outcome>, // 2
    pub expires_at:       i64,          // 8
    pub created_at:       i64,          // 8
    /// Number of UserPosition accounts that still have unclaimed shares.
    /// Vault can only close when this reaches 0.
    pub claims_remaining: u64,          // 8
    pub bump:             u8,           // 1
    pub vault_bump:       u8,           // 1
}

impl Market {
    pub const LEN: usize = 8     // discriminator
        + 8 + 1                  // game_id, market_index
        + (4 + 128)              // question string
        + 8 + 8 + 8              // yes_supply, no_supply, total_volume
        + 1 + 2                  // resolved, outcome
        + 8 + 8                  // expires_at, created_at
        + 8                      // claims_remaining
        + 1 + 1                  // bumps
        + 32;                    // headroom
}

// ─────────────────────────────────────────────────────────────────────────────
// UserPosition  (PDA: ["position", market_key, user_key])
// ─────────────────────────────────────────────────────────────────────────────
// Never auto-closed. Only closeable by the user after claimed == true.

#[account]
pub struct UserPosition {
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
    pub const LEN: usize = 8 + 32 + 8 + 1 + 8 + 8 + 1 + 1 + 16; // +16 headroom
}
