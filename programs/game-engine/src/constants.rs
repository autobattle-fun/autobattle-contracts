/// Seeds
pub const REGISTRY_SEED: &[u8] = b"registry";
pub const GAME_SEED: &[u8]     = b"game";
pub const PAWN_SEED: &[u8]     = b"pawn";
pub const VRF_SEED: &[u8]      = b"vrf_request";

/// Cross-program IDs
/// Replace with actual deployed prediction-market program ID before deploy.
use anchor_lang::prelude::Pubkey;
pub const PREDICTION_MARKET_PROGRAM_ID: Pubkey = solana_program::pubkey!(
    "H76M7bbm6zwE464xkabF5MWbciwZqK9FmZYf4omaqnQH"
);

/// Ludo board
pub const BOARD_TRACK_LEN: u8  = 52; // main track squares
pub const HOME_STRETCH_LEN: u8 = 5;  // coloured home stretch
pub const HOME_POSITION: u8    = 99; // sentinel: pawn is home
pub const STARTING_SQUARE: u8  = 255;  // off-board sentinel

pub const PAWNS_PER_PLAYER: u8 = 4;
pub const TOTAL_PLAYERS: u8    = 4;
pub const TOTAL_PAWNS: u8      = 16; // 4 players × 4 pawns

/// Each player's entry square on the main track (0-indexed, clockwise)
pub const ENTRY_SQUARES: [u8; 4] = [0, 13, 26, 39];

/// Safe squares (stars + home column entries) — cannot be captured on these
pub const SAFE_SQUARES: [u8; 8] = [0, 8, 13, 21, 26, 34, 39, 47];

/// Timing
pub const DEFAULT_COOLDOWN_SECS: i64 = 300; // 5 minutes
pub const MARKET_EXPIRY_GRACE: i64   = 7200; // 2 hours after game ends
pub const MAX_TURNS_WITHOUT_MOVE: u8 = 3;    // forfeit after 3 consecutive no-moves
