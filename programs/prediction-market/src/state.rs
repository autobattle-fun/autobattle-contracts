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
    pub market_index:     u8,           // 1  
    pub question:         String,       // 4 + 128
    pub yes_supply:       u64,          // 8
    pub no_supply:        u64,          // 8
    pub total_volume:     u64,          // 8
    pub resolved:         bool,         // 1
    pub outcome:          Option<Outcome>, // 2
    pub expires_at:       i64,          // 8
    pub created_at:       i64,          // 8
    pub fee_balance:      u64,          // 8  <-- NEW FIELD (Tracks uncollected fees)
    pub bump:             u8,           // 1
    pub vault_bump:       u8,           // 1
    pub lp_withdrawn: bool,
}

impl Market {
    pub const LEN: usize = 
          8     // Anchor discriminator
        + 8     // game_id
        + 1     // market_index
        + 132   // question (4 byte string length prefix + 128 bytes data)
        + 8     // yes_supply
        + 8     // no_supply
        + 8     // total_volume
        + 1     // resolved (bool)
        + 2     // outcome (Option<Outcome> = 1 byte discriminant + 1 byte enum payload)
        + 8     // expires_at
        + 8     // created_at
        + 8     // fee_balance
        + 1     // bump
        + 1     // vault_bump
        + 1     // lp_withdrawn
        + 31;   // headroom padding to reach exactly 234 bytes
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
