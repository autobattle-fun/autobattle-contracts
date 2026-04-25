use anchor_lang::prelude::Pubkey;

pub const REGISTRY_SEED: &[u8] = b"registry";
pub const GAME_SEED: &[u8]     = b"game";
pub const VRF_SEED: &[u8]      = b"vrf_request";
pub const MARKET_SEED: &[u8]   = b"market";

pub const PREDICTION_MARKET_PROGRAM_ID: Pubkey = solana_program::pubkey!(
    "H76M7bbm6zwE464xkabF5MWbciwZqK9FmZYf4omaqnQH"
);
pub const ADMIN_PUBKEY: Pubkey = solana_program::pubkey!(
    "G2eWnQNwc1wrrgE78NcjmLBXXT9h2s9iUwAM1C8kpFzK"
);

pub const DEFAULT_COOLDOWN_SECS: i64 = 300;