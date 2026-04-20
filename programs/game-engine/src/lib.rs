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

    pub fn init_game<'info>(
        ctx: Context<'_, '_, 'info, 'info, InitGame<'info>>, 
        agent_red: Pubkey, 
        agent_blue: Pubkey, 
    ) -> Result<()> {
        instructions::game::init_game(ctx, agent_red, agent_blue)
    }

    pub fn request_vrf(
        ctx: Context<RequestRoll>,
        roll_type: u8,
    ) -> Result<()> {
        instructions::game::request_vrf(ctx, roll_type)
    }

    pub fn fulfill_vrf(ctx: Context<FulfillRoll>) -> Result<()> {
        instructions::game::fulfill_vrf(ctx)
    }

    pub fn stay(ctx: Context<Action>, player: state::Color) -> Result<()> {
        instructions::game::stay(ctx, player)
    }

    pub fn resolve_round<'info>(ctx: Context<'_, '_, 'info, 'info, ResolveRound<'info>>) -> Result<()> {
        instructions::game::resolve_round(ctx)
    }

    pub fn unlock_upgrade(ctx: Context<UnlockUpgrade>) -> Result<()> {
        instructions::game::unlock_upgrade(ctx)
    }

    // pub fn mock_fulfill_vrf(ctx: Context<MockFulfillRoll>, random_bytes: [u8; 2]) -> Result<()> {
    //     instructions::game::mock_fulfill_vrf(ctx, random_bytes)
    // }

    // pub fn mock_request_vrf(ctx: Context<RequestRoll>, roll_type: u8) -> Result<()> {
    //     instructions::game::mock_request_vrf(ctx, roll_type)
    // }
}