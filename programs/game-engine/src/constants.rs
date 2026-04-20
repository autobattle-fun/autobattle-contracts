use anchor_lang::prelude::Pubkey;

/// Seeds
pub const REGISTRY_SEED: &[u8] = b"registry";
pub const GAME_SEED: &[u8]     = b"game";
pub const VRF_SEED: &[u8]      = b"vrf_request";

/// Cross-program IDs
pub const PREDICTION_MARKET_PROGRAM_ID: Pubkey = solana_program::pubkey!(
    "H76M7bbm6zwE464xkabF5MWbciwZqK9FmZYf4omaqnQH"
);

/// Timing
pub const DEFAULT_COOLDOWN_SECS: i64 = 300; // 5 minutes