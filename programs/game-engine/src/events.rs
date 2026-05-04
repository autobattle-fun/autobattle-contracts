use anchor_lang::prelude::*;
use crate::state::Color;

#[event]
pub struct GameInitialised {
    pub game_id: u64,
    pub agent_red: Pubkey,
    pub agent_blue: Pubkey,
    pub starts_at: i64,
}

#[event]
pub struct VrfRequested {
    pub game_id: u64,
    pub player: Color,
    pub roll_type: u8,
    pub timestamp: i64,
}

#[event]
pub struct CardsDealt {
    pub game_id: u64,
    pub p1_score: u8,
    pub p2_score: u8,
    pub is_final_reveal: bool,
}

#[event]
pub struct PlayerHit {
    pub game_id: u64,
    pub player: Color,
    pub new_score: u8,
}

#[event]
pub struct PlayerStayed {
    pub game_id: u64,
    pub player: Color,
}

#[event]
pub struct RoundResolved {
    pub game_id: u64,
    pub round_number: u8,
    pub p1_hp: u8,
    pub p2_hp: u8,
    pub damage_dealt: u8,
}

#[event]
pub struct GameEnded {
    pub game_id: u64,
    pub winner: Color,
    pub winner_agent: Pubkey,
    pub total_rounds: u8,
    pub ended_at: i64,
}

#[event]
pub struct UpgradeUnlocked {
    pub game_id: u64,
    pub timestamp: i64,
}