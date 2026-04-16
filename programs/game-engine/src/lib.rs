use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("GxLT8QMUw6cVT6HQBu2c2zepbQnhUWr4VPEB2vfggE2e");

#[program]
pub mod game_engine {
    use super::*;

    // ── Registry ──────────────────────────────────────────────────────────────

    pub fn initialize_registry(
        ctx: Context<InitializeRegistry>,
        cooldown_duration: i64,
    ) -> Result<()> {
        instructions::registry::initialize_registry(ctx, cooldown_duration)
    }

    pub fn update_cooldown(
        ctx: Context<UpdateCooldown>,
        new_duration: i64,
    ) -> Result<()> {
        instructions::registry::update_cooldown(ctx, new_duration)
    }

    // ── Game lifecycle ────────────────────────────────────────────────────────

    /// Crank: start a new game after cooldown elapses.
    /// Pass previous GameState PDA as remaining_accounts[0] when game_count > 0.
    pub fn init_game<'info>(
        ctx: Context<'_, '_, 'info, 'info, InitGame<'info>>, 
        agent_red: Pubkey, 
        agent_blue: Pubkey, 
        agent_yellow: Pubkey, 
        agent_green: Pubkey
    ) -> Result<()> {
        instructions::game::init_game(ctx, agent_red, agent_blue, agent_yellow, agent_green)
    }

    /// Active agent commits to a pawn and requests Switchboard randomness.
    pub fn request_roll(
        ctx: Context<RequestRoll>,
        pawn_id: state::PawnId,
    ) -> Result<()> {
        instructions::game::request_roll(ctx, pawn_id)
    }

    /// Switchboard oracle CPI callback: reads randomness, moves pawn, checks win.
    pub fn fulfill_roll(ctx: Context<FulfillRoll>) -> Result<()> {
        instructions::game::fulfill_roll(ctx)
    }

    /// Finalise game: CPI into prediction-market to resolve all 4 win markets.
    /// Pass 4 Market PDAs (one per color) as remaining_accounts[0..3].
    pub fn end_game<'info>(ctx: Context<'_, '_, 'info, 'info, EndGame<'info>>) -> Result<()> {
        instructions::game::end_game(ctx)
    }

    /// Called via CPI from prediction-market once all claims_remaining == 0.
    pub fn unlock_upgrade(ctx: Context<UnlockUpgrade>) -> Result<()> {
        instructions::game::unlock_upgrade(ctx)
    }
}
