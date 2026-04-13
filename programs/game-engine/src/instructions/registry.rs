use anchor_lang::prelude::*;
use crate::{constants::*, errors::LudoError, state::Registry};

// ─────────────────────────────────────────────────────────────────────────────
// initialize_registry
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitializeRegistry<'info> {
    #[account(
        init,
        seeds = [REGISTRY_SEED],
        bump,
        payer = authority,
        space = Registry::LEN,
    )]
    pub registry: Account<'info, Registry>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn initialize_registry(
    ctx: Context<InitializeRegistry>,
    cooldown_duration: i64,
) -> Result<()> {
    let reg = &mut ctx.accounts.registry;
    reg.authority         = ctx.accounts.authority.key();
    reg.game_count        = 0;
    reg.current_game_id   = 0;
    reg.cooldown_duration = cooldown_duration;
    reg.game_active       = false;
    reg.bump              = ctx.bumps.registry;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// update_cooldown
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct UpdateCooldown<'info> {
    #[account(
        mut,
        seeds = [REGISTRY_SEED],
        bump = registry.bump,
        has_one = authority @ LudoError::UnauthorizedAgent,
    )]
    pub registry: Account<'info, Registry>,

    pub authority: Signer<'info>,
}

pub fn update_cooldown(
    ctx: Context<UpdateCooldown>,
    new_duration: i64,
) -> Result<()> {
    require!(!ctx.accounts.registry.game_active, LudoError::GameInProgress);
    ctx.accounts.registry.cooldown_duration = new_duration;
    Ok(())
}
