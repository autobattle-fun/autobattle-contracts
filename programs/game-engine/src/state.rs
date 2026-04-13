use anchor_lang::prelude::*;
use crate::constants::*;

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum GamePhase {
    Cooldown,      // waiting 5 min before next game
    AwaitingRoll,  // active agent must call request_roll
    AwaitingVRF,   // VRF request in-flight
    Ended,         // winner decided, markets being resolved
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Red    = 0,
    Blue   = 1,
    Yellow = 2,
    Green  = 3,
}

impl Color {
    pub fn from_index(i: u8) -> Option<Self> {
        match i {
            0 => Some(Color::Red),
            1 => Some(Color::Blue),
            2 => Some(Color::Yellow),
            3 => Some(Color::Green),
            _ => None,
        }
    }

    pub fn entry_square(&self) -> u8 {
        ENTRY_SQUARES[*self as usize]
    }
}

/// PawnId encodes both color and index (r1–g4).
/// Stored as a u8: high nibble = color (0-3), low nibble = pawn index (0-3).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PawnId(pub u8);

impl PawnId {
    pub fn new(color: Color, index: u8) -> Self {
        debug_assert!(index < 4);
        PawnId((color as u8) << 4 | (index & 0x0F))
    }

    pub fn color(&self) -> Color {
        Color::from_index(self.0 >> 4).unwrap()
    }

    pub fn index(&self) -> u8 {
        self.0 & 0x0F
    }

    /// Flat index 0-15 for board_snapshot arrays.
    pub fn flat(&self) -> usize {
        (self.color() as usize) * 4 + self.index() as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registry  (PDA: ["registry"])
// ─────────────────────────────────────────────────────────────────────────────

#[account]
pub struct Registry {
    /// Protocol authority — controls cooldown updates and upgrade approvals.
    pub authority: Pubkey,          // 32
    /// Monotonically increasing. game_id of the next game to be created.
    pub game_count: u64,            // 8
    /// game_id of the currently active (or cooldown) game.
    pub current_game_id: u64,       // 8
    /// Seconds between game_ended_at and next init_game being allowed.
    pub cooldown_duration: i64,     // 8
    /// True while a game is InProgress — blocks cooldown updates.
    pub game_active: bool,          // 1
    pub bump: u8,                   // 1
}

impl Registry {
    pub const LEN: usize = 8 + 32 + 8 + 8 + 8 + 1 + 1 + 64; // +64 headroom
}

// ─────────────────────────────────────────────────────────────────────────────
// GameState  (PDA: ["game", game_id.to_le_bytes()])
// ─────────────────────────────────────────────────────────────────────────────

#[account]
pub struct GameState {
    pub game_id: u64,               // 8

    // Agents (one pubkey per color, index matches Color enum)
    pub agents: [Pubkey; 4],        // 4 × 32 = 128

    // Turn state
    pub phase: GamePhase,           // 1
    pub active_player: Color,       // 1  — whose turn it is
    pub turn_number: u32,           // 4
    pub consecutive_no_moves: u8,   // 4  — tracks skipped turns for forfeit

    // VRF linkage — set in request_roll, cleared in fulfill_roll
    pub pending_request_id: [u8; 32], // 32
    pub pending_pawn_id: PawnId,    // 1  — pawn committed before seeing roll

    // Board: square number for each pawn (flat index, see PawnId::flat())
    // 0           = in yard (not yet entered)
    // 1-52        = main track position
    // 53-57       = home stretch (color-specific)
    // HOME(99)    = reached home
    pub pawn_positions: [u8; 16],   // 16

    // How many pawns each player has safely home
    pub home_counts: [u8; 4],       // 4

    // Winner — set on end_game
    pub winner: Option<Color>,      // 2  (Option<u8> = 2 bytes in Anchor)

    // Timing
    pub created_at: i64,            // 8
    pub ended_at: i64,              // 8  — 0 until game ends
    pub next_game_starts_at: i64,   // 8

    // Safety flag: set true while any prediction market for this game
    // has unclaimed positions. Prevents upgrade authority from pushing
    // a broken claim instruction mid-game.
    pub upgrade_locked: bool,       // 1

    pub bump: u8,                   // 1
}

impl GameState {
    pub const LEN: usize = 8   // discriminator
        + 8                    // game_id
        + 128                  // agents
        + 1 + 1 + 4 + 1        // phase, active_player, turn_number, no_moves
        + 32 + 1               // pending_request_id, pending_pawn_id
        + 16                   // pawn_positions
        + 4                    // home_counts
        + 2                    // winner
        + 8 + 8 + 8            // timing
        + 1 + 1                // upgrade_locked, bump
        + 64;                  // headroom

    pub fn agent_for(&self, color: Color) -> Pubkey {
        self.agents[color as usize]
    }

    pub fn pawn_pos(&self, pawn: PawnId) -> u8 {
        self.pawn_positions[pawn.flat()]
    }

    pub fn set_pawn_pos(&mut self, pawn: PawnId, pos: u8) {
        self.pawn_positions[pawn.flat()] = pos;
    }

    /// Returns true if pawn is sitting in the yard (never entered).
    pub fn is_in_yard(&self, pawn: PawnId) -> bool {
        self.pawn_pos(pawn) == STARTING_SQUARE
    }

    /// Returns true if pawn is home.
    pub fn is_home(&self, pawn: PawnId) -> bool {
        self.pawn_pos(pawn) == HOME_POSITION
    }

    /// Board snapshot for events — clone of pawn_positions.
    pub fn snapshot(&self) -> [u8; 16] {
        self.pawn_positions
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VRFRequest  (PDA: ["vrf_request", game_id, request_id])
// ─────────────────────────────────────────────────────────────────────────────
// Short-lived. Created in request_roll, closed in fulfill_roll.

#[account]
pub struct VRFRequest {
    pub game_id:      u64,       // 8
    /// = Switchboard RandomnessAccount pubkey (used as request_id)
    pub request_id:   [u8; 32],  // 32
    /// Explicit Switchboard account pubkey — verified in fulfill_roll
    pub sb_account:   Pubkey,    // 32
    pub pawn_id:      PawnId,    // 1  — pawn committed before seeing roll
    pub player:       Color,     // 1
    pub consumed:     bool,      // 1  — replay protection
    pub requested_at: i64,       // 8
    pub bump:         u8,        // 1
}

impl VRFRequest {
    pub const LEN: usize = 8 + 8 + 32 + 32 + 1 + 1 + 1 + 8 + 1 + 32; // +32 headroom
}
