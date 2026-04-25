use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Red = 0,
    Blue = 1,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum GamePhase {
    AwaitingInitialDeal,
    AwaitingHitVRF,
    AwaitingAction,
    AwaitingFinalRevealVRF,
    ReadyToResolve,
    AwaitingTiebreakerVRF,
    Ended,
}

#[account]
pub struct Registry {
    pub authority: Pubkey,
    pub game_count: u64,
    pub current_game_id: u64,
    pub cooldown_duration: i64,
    pub game_active: bool,
    pub bump: u8,
}

impl Registry {
    pub const LEN: usize = 8 + 32 + 8 + 8 + 8 + 1 + 1;
}

#[account]
pub struct GameState {
    pub game_id: u64,
    pub agent_red: Pubkey,
    pub agent_blue: Pubkey,
    pub p1_hp: u8,
    pub p2_hp: u8,
    pub p1_score: u8,
    pub p2_score: u8,
    pub p1_last_card: u8,
    pub p2_last_card: u8,
    pub p1_aces: u8,
    pub p2_aces: u8,
    pub p1_stayed: bool,
    pub p2_stayed: bool,
    pub round_number: u8,
    pub phase: GamePhase,
    pub active_player: Color,
    pub winner: Option<Color>,
    pub pending_commit_slot: u64,
    pub created_at: i64,
    pub upgrade_locked: bool,
    pub bump: u8,
}

impl GameState {
    pub const LEN: usize = 8 + 8 + 32 + 32 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 2 + 8 + 8 + 1 + 1 + 32;

    pub fn agent_for(&self, color: Color) -> Pubkey {
        match color {
            Color::Red => self.agent_red,
            Color::Blue => self.agent_blue,
        }
    }
}

#[account]
pub struct VRFRequest {
    pub game_id: u64,
    pub commit_slot: u64,
    pub sb_account: Pubkey,
    pub player: Color,
    pub roll_type: u8,
    pub consumed: bool,
    pub bump: u8,
}

impl VRFRequest {
    pub const LEN: usize = 8 + 8 + 8 + 32 + 1 + 1 + 1 + 1;
}