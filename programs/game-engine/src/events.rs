use anchor_lang::prelude::*;
use crate::state::{Color, PawnId};

/// Emitted when a new game is initialised.
#[event]
pub struct GameInitialised {
    pub game_id: u64,
    pub agent_red: Pubkey,
    pub agent_blue: Pubkey,
    pub agent_yellow: Pubkey,
    pub agent_green: Pubkey,
    pub starts_at: i64,
}

/// Emitted when an agent requests a VRF roll.
#[event]
pub struct RollRequested {
    pub game_id: u64,
    pub player: Color,
    pub pawn_id: PawnId,
    pub request_id: [u8; 32],
    pub timestamp: i64,
}

/// Emitted when the VRF is fulfilled and a pawn moves.
#[event]
pub struct PawnMoved {
    pub game_id: u64,
    pub player: Color,
    pub pawn_id: PawnId,
    pub roll: u8,
    pub from_square: u8,
    pub to_square: u8,
    pub captured_pawn: Option<PawnId>, // set if this move sent an opponent home
    pub turn_number: u32,
    pub timestamp: i64,
}

/// Emitted when a pawn reaches the home position.
#[event]
pub struct PawnReachedHome {
    pub game_id: u64,
    pub player: Color,
    pub pawn_id: PawnId,
    pub pawns_home_count: u8, // how many of this player's pawns are home after this move
}

/// Emitted when a capture happens — useful for live market proposals.
#[event]
pub struct PawnCaptured {
    pub game_id: u64,
    pub capturing_player: Color,
    pub captured_player: Color,
    pub captured_pawn_id: PawnId,
    pub square: u8,
    pub turn_number: u32,
}

/// Emitted when the game ends.
#[event]
pub struct GameEnded {
    pub game_id: u64,
    pub winner: Color,
    pub winner_agent: Pubkey,
    pub turn_count: u32,
    pub ended_at: i64,
    pub next_game_starts_at: i64,
}

/// Emitted every turn — drives live market proposals from the backend.
#[event]
pub struct TurnAdvanced {
    pub game_id: u64,
    pub turn_number: u32,
    pub active_player: Color,
    /// Snapshot of all 16 pawn squares for the indexer.
    pub board_snapshot: [u8; 16],
}
