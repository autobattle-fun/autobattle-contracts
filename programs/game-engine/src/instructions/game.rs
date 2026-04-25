use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use switchboard_on_demand::accounts::RandomnessAccountData;

use crate::{constants::*, errors::GameError, events::*, state::*};

// ── Contexts ─────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitGame<'info> {
    #[account(mut, seeds = [REGISTRY_SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,
    #[account(
        init,
        seeds = [GAME_SEED, &(registry.game_count + 1).to_le_bytes()],
        bump,
        payer = crank,
        space = GameState::LEN,
    )]
    pub game_state: Account<'info, GameState>,
    #[account(mut)]
    pub crank: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(roll_type: u8)]
pub struct RequestRoll<'info> {
    #[account(mut, seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()], bump = game_state.bump)]
    pub game_state: Account<'info, GameState>,
    #[account(
        init,
        seeds = [VRF_SEED, &game_state.game_id.to_le_bytes()],
        bump,
        payer = agent,
        space = VRFRequest::LEN,
    )]
    pub vrf_request: Account<'info, VRFRequest>,
    /// CHECK: Data validated via RandomnessAccountData::parse in instruction logic
    pub randomness_account: AccountInfo<'info>,
    #[account(mut)]
    pub agent: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct FulfillRoll<'info> {
    #[account(mut, seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()], bump = game_state.bump)]
    pub game_state: Account<'info, GameState>,
    #[account(
        mut,
        seeds = [VRF_SEED, &game_state.game_id.to_le_bytes()],
        bump = vrf_request.bump,
        close = crank,
    )]
    pub vrf_request: Account<'info, VRFRequest>,
    /// CHECK: Data validated via RandomnessAccountData::parse in instruction logic
    pub randomness_account: AccountInfo<'info>,
    #[account(mut)]
    pub crank: Signer<'info>,
}

#[derive(Accounts)]
pub struct Action<'info> {
    #[account(mut, seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()], bump = game_state.bump)]
    pub game_state: Account<'info, GameState>,
    #[account(mut)]
    pub agent: Signer<'info>,
}

#[derive(Accounts)]
pub struct ResolveRound<'info> {
    #[account(mut, seeds = [REGISTRY_SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,
    #[account(mut, seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()], bump = game_state.bump)]
    pub game_state: Account<'info, GameState>,
    #[account(mut)]
    pub crank: Signer<'info>,
}

// FIX: Programs cannot be Signers. unlock_upgrade is now called by the admin keypair,
// which is the only trustworthy off-chain authority available.
#[derive(Accounts)]
pub struct UnlockUpgrade<'info> {
    #[account(mut, seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()], bump = game_state.bump)]
    pub game_state: Account<'info, GameState>,
    #[account(mut, address = ADMIN_PUBKEY @ GameError::UnauthorizedCrank)]
    pub authority: Signer<'info>,
}

// ── Instruction Logic ────────────────────────────────────────────────────────

pub fn init_game<'info>(
    ctx: Context<'_, '_, 'info, 'info, InitGame<'info>>,
    agent_red: Pubkey,
    agent_blue: Pubkey,
) -> Result<()> {
    let reg   = &mut ctx.accounts.registry;
    let clock = Clock::get()?;

    let new_game_id = reg.game_count.checked_add(1).ok_or(GameError::Overflow)?;
    reg.game_count      = new_game_id;
    reg.current_game_id = new_game_id;
    reg.game_active     = true;

    let gs = &mut ctx.accounts.game_state;
    gs.game_id              = new_game_id;
    gs.agent_red            = agent_red;
    gs.agent_blue           = agent_blue;
    gs.p1_hp                = 10;
    gs.p2_hp                = 10;
    gs.p1_score             = 0;
    gs.p2_score             = 0;
    gs.p1_aces              = 0;
    gs.p2_aces              = 0;
    gs.p1_stayed            = false;
    gs.p2_stayed            = false;
    gs.round_number         = 1;
    gs.phase                = GamePhase::AwaitingInitialDeal;
    gs.active_player        = Color::Red;
    gs.winner               = None;
    gs.created_at           = clock.unix_timestamp;
    gs.upgrade_locked       = true;
    gs.bump                 = ctx.bumps.game_state;

    emit!(GameInitialised {
        game_id: new_game_id,
        agent_red,
        agent_blue,
        starts_at: clock.unix_timestamp,
    });

    Ok(())
}

pub fn request_vrf(ctx: Context<RequestRoll>, roll_type: u8) -> Result<()> {
    let gs    = &mut ctx.accounts.game_state;
    let clock = Clock::get()?;

    if roll_type == 0 {
        require!(gs.phase == GamePhase::AwaitingInitialDeal, GameError::InvalidPhase);
    } else if roll_type == 1 {
        require!(gs.phase == GamePhase::AwaitingAction, GameError::InvalidPhase);
        require!(ctx.accounts.agent.key() == gs.agent_for(gs.active_player), GameError::NotYourTurn);
        let score = if gs.active_player == Color::Red { gs.p1_score } else { gs.p2_score };
        require!(score <= 21, GameError::Over21CannotHit);
    } else if roll_type == 2 {
        require!(gs.phase == GamePhase::AwaitingFinalRevealVRF, GameError::InvalidPhase);
    } else if roll_type == 3 {
        require!(gs.phase == GamePhase::AwaitingTiebreakerVRF, GameError::InvalidPhase);
    }

    // FIX: .unwrap() replaced with proper error propagation to avoid panics on bad input.
    // Owner constraint on the account struct ensures it belongs to Switchboard.
    let randomness_data = RandomnessAccountData::parse(
        ctx.accounts.randomness_account.data.borrow()
    ).map_err(|_| GameError::InvalidRandomnessAccount)?;

    require!(randomness_data.seed_slot == clock.slot - 1, GameError::RandomnessExpired);
    require!(randomness_data.get_value(clock.slot).is_err(), GameError::RandomnessAlreadyRevealed);

    gs.pending_commit_slot = randomness_data.seed_slot;

    gs.phase = match roll_type {
        0 => GamePhase::AwaitingInitialDeal,
        1 => GamePhase::AwaitingHitVRF,
        2 => GamePhase::AwaitingFinalRevealVRF,
        _ => GamePhase::AwaitingTiebreakerVRF,
    };

    let vrf = &mut ctx.accounts.vrf_request;
    vrf.game_id     = gs.game_id;
    vrf.commit_slot = randomness_data.seed_slot;
    vrf.sb_account  = ctx.accounts.randomness_account.key();
    vrf.player      = gs.active_player;
    vrf.roll_type   = roll_type;
    vrf.consumed    = false;
    vrf.bump        = ctx.bumps.vrf_request;

    emit!(VrfRequested {
        game_id: gs.game_id,
        player: gs.active_player,
        roll_type,
        timestamp: clock.unix_timestamp,
    });

    Ok(())
}

pub fn fulfill_vrf(ctx: Context<FulfillRoll>) -> Result<()> {
    let clock = Clock::get()?;

    require!(
        ctx.accounts.randomness_account.key() == ctx.accounts.vrf_request.sb_account,
        GameError::InvalidRandomnessAccount
    );

    // FIX: .unwrap() replaced with proper error propagation.
    let randomness_data = RandomnessAccountData::parse(
        ctx.accounts.randomness_account.data.borrow()
    ).map_err(|_| GameError::InvalidRandomnessAccount)?;

    require!(
        randomness_data.seed_slot == ctx.accounts.vrf_request.commit_slot,
        GameError::RandomnessExpired
    );

    let revealed_random_value = randomness_data
        .get_value(clock.slot)
        .map_err(|_| GameError::RandomnessNotResolved)?;

    ctx.accounts.vrf_request.consumed = true;

    let gs    = &mut ctx.accounts.game_state;
    let inner = &mut **gs;

    if ctx.accounts.vrf_request.roll_type == 0 {
        inner.p1_last_card = apply_card(&mut inner.p1_score, &mut inner.p1_aces, revealed_random_value[0]);
        inner.p2_last_card = apply_card(&mut inner.p2_score, &mut inner.p2_aces, revealed_random_value[1]);
        inner.phase = GamePhase::AwaitingAction;

        emit!(CardsDealt {
            game_id: inner.game_id,
            p1_score: inner.p1_score,
            p2_score: inner.p2_score,
            is_final_reveal: false,
        });

    } else if ctx.accounts.vrf_request.roll_type == 1 {
        if inner.active_player == Color::Red {
            inner.p1_last_card = apply_card(&mut inner.p1_score, &mut inner.p1_aces, revealed_random_value[0]);
            if inner.p1_score >= 21 { inner.p1_stayed = true; }
        } else {
            inner.p2_last_card = apply_card(&mut inner.p2_score, &mut inner.p2_aces, revealed_random_value[0]);
            if inner.p2_score >= 21 { inner.p2_stayed = true; }
        }

        emit!(PlayerHit {
            game_id: inner.game_id,
            player: inner.active_player,
            new_score: if inner.active_player == Color::Red { inner.p1_score } else { inner.p2_score },
        });

        if inner.active_player == Color::Red && !inner.p2_stayed {
            inner.active_player = Color::Blue;
        } else if inner.active_player == Color::Blue && !inner.p1_stayed {
            inner.active_player = Color::Red;
        }

        inner.phase = if inner.p1_stayed && inner.p2_stayed {
            GamePhase::AwaitingFinalRevealVRF
        } else {
            GamePhase::AwaitingAction
        };

    } else if ctx.accounts.vrf_request.roll_type == 2 {
        inner.p1_last_card = apply_card(&mut inner.p1_score, &mut inner.p1_aces, revealed_random_value[0]);
        inner.p2_last_card = apply_card(&mut inner.p2_score, &mut inner.p2_aces, revealed_random_value[1]);
        inner.phase = GamePhase::ReadyToResolve;

        emit!(CardsDealt {
            game_id: inner.game_id,
            p1_score: inner.p1_score,
            p2_score: inner.p2_score,
            is_final_reveal: true,
        });

    } else if ctx.accounts.vrf_request.roll_type == 3 {
        // FIX: Tiebreaker scores were already reset in resolve_round before entering
        // AwaitingTiebreakerVRF, so cards are dealt fresh onto clean state here.
        inner.p1_last_card = apply_card(&mut inner.p1_score, &mut inner.p1_aces, revealed_random_value[0]);
        inner.p2_last_card = apply_card(&mut inner.p2_score, &mut inner.p2_aces, revealed_random_value[1]);
        inner.phase = GamePhase::ReadyToResolve;

        emit!(CardsDealt {
            game_id: inner.game_id,
            p1_score: inner.p1_score,
            p2_score: inner.p2_score,
            is_final_reveal: true,
        });
    }

    Ok(())
}

pub fn stay(ctx: Context<Action>, player: Color) -> Result<()> {
    let gs = &mut ctx.accounts.game_state;
    require!(gs.phase == GamePhase::AwaitingAction, GameError::InvalidPhase);
    require!(ctx.accounts.agent.key() == gs.agent_for(player), GameError::NotYourTurn);

    if player == Color::Red {
        gs.p1_stayed = true;
        if !gs.p2_stayed { gs.active_player = Color::Blue; }
    } else {
        gs.p2_stayed = true;
        if !gs.p1_stayed { gs.active_player = Color::Red; }
    }

    emit!(PlayerStayed {
        game_id: gs.game_id,
        player,
    });

    if gs.p1_stayed && gs.p2_stayed {
        gs.phase = GamePhase::AwaitingFinalRevealVRF;
    }

    Ok(())
}

pub fn resolve_round<'info>(ctx: Context<'_, '_, 'info, 'info, ResolveRound<'info>>) -> Result<()> {
    let gs = &mut ctx.accounts.game_state;
    require!(gs.phase == GamePhase::ReadyToResolve, GameError::InvalidPhase);

    let p1_diff = gs.p1_score.abs_diff(21);
    let p2_diff = gs.p2_score.abs_diff(21);

    // Tiebreaker: reset all round state before entering sudden death so the
    // tiebreaker VRF deals fresh cards rather than adding on top of existing scores.
    if p1_diff == p2_diff {
        msg!("Tie detected! Entering Sudden Death.");
        gs.p1_score   = 0;
        gs.p2_score   = 0;
        gs.p1_aces    = 0;
        gs.p2_aces    = 0;
        gs.p1_stayed  = false;
        gs.p2_stayed  = false;
        gs.phase = GamePhase::AwaitingTiebreakerVRF;
        return Ok(());
    }

    // FIX: Shift is capped at 7 to prevent u8 overflow on high round numbers (max damage = 128).
    let shift = gs.round_number.saturating_sub(1).min(7);
    let damage: u8 = 1u8 << shift;

    let round_winner = if p1_diff < p2_diff {
        gs.p2_hp = gs.p2_hp.saturating_sub(damage);
        msg!("Red wins round! Dealt {} damage.", damage);
        Color::Red
    } else {
        gs.p1_hp = gs.p1_hp.saturating_sub(damage);
        msg!("Blue wins round! Dealt {} damage.", damage);
        Color::Blue
    };

    // Verify the round market PDA before resolving to prevent a malicious caller
    // from passing an arbitrary account.
    if let Some(round_market_info) = ctx.remaining_accounts.get(0) {
        let market_index: u8 = gs.round_number.saturating_sub(1);
        let (expected_pda, _) = Pubkey::find_program_address(
            &[MARKET_SEED, &gs.game_id.to_le_bytes(), &[market_index]],
            &PREDICTION_MARKET_PROGRAM_ID,
        );
        require!(round_market_info.key() == expected_pda, GameError::InvalidMarketAccount);
        resolve_market_cpi(gs, round_market_info, round_winner)?;
    }

    if gs.p1_hp == 0 || gs.p2_hp == 0 {
        gs.phase  = GamePhase::Ended;
        gs.winner = Some(round_winner);
        ctx.accounts.registry.game_active = false;

        emit!(GameEnded {
            game_id:      gs.game_id,
            winner:       gs.winner.unwrap(),
            winner_agent: gs.agent_for(gs.winner.unwrap()),
            total_rounds: gs.round_number,
            ended_at:     Clock::get()?.unix_timestamp,
        });

        // Verify the main (match-winner) market PDA before resolving.
        if let Some(main_market_info) = ctx.remaining_accounts.get(1) {
            let (expected_pda, _) = Pubkey::find_program_address(
                &[MARKET_SEED, &gs.game_id.to_le_bytes(), &[255u8]], // index 255 = match winner market
                &PREDICTION_MARKET_PROGRAM_ID,
            );
            require!(main_market_info.key() == expected_pda, GameError::InvalidMarketAccount);
            resolve_market_cpi(gs, main_market_info, gs.winner.unwrap())?;
        }
    } else {
        gs.p1_score   = 0;
        gs.p2_score   = 0;
        gs.p1_aces    = 0;
        gs.p2_aces    = 0;
        gs.p1_stayed  = false;
        gs.p2_stayed  = false;
        gs.round_number += 1;
        gs.phase = GamePhase::AwaitingInitialDeal;
    }

    Ok(())
}

pub fn unlock_upgrade(ctx: Context<UnlockUpgrade>) -> Result<()> {
    ctx.accounts.game_state.upgrade_locked = false;
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Draws a card from a random byte, applies blackjack ace-reduction logic,
/// updates score and ace count in-place, and returns the raw card face value.
/// FIX: Uses saturating_add to prevent u8 overflow before ace reduction runs.
fn apply_card(score: &mut u8, aces: &mut u8, random_byte: u8) -> u8 {
    let raw = (random_byte % 13) + 1;
    let value = if raw == 1 {
        *aces += 1;
        11u8
    } else if raw > 10 {
        10u8
    } else {
        raw
    };

    *score = score.saturating_add(value);

    while *score > 21 && *aces > 0 {
        *score -= 10;
        *aces -= 1;
    }

    raw
}

/// Resolves a prediction market via CPI, signing with the game_state PDA.
///
/// IMPORTANT: The prediction market's ResolveMarket context must accept the
/// game_state PDA as a valid authority (not just ADMIN_PUBKEY). Update the
/// prediction market's ResolveMarket constraint to:
///   constraint = authority.key() == ADMIN_PUBKEY ||
///                authority.key() == expected_game_pda
fn resolve_market_cpi<'info>(
    gs: &Account<'info, GameState>,
    market_info: &AccountInfo<'info>,
    winner: Color,
) -> Result<()> {
    let disc = anchor_discriminator(b"global:resolve_market");
    let outcome_byte: u8 = if winner == Color::Red { 0 } else { 1 };

    let mut ix_data = disc.to_vec();
    ix_data.push(outcome_byte);

    let game_id_bytes = gs.game_id.to_le_bytes();
    let signer_seeds: &[&[&[u8]]] = &[&[GAME_SEED, &game_id_bytes, &[gs.bump]]];

    let ix = solana_program::instruction::Instruction {
        program_id: PREDICTION_MARKET_PROGRAM_ID,
        accounts: vec![
            solana_program::instruction::AccountMeta::new(market_info.key(), false),
            solana_program::instruction::AccountMeta::new_readonly(gs.key(), true),
        ],
        data: ix_data,
    };

    invoke_signed(&ix, &[market_info.clone(), gs.to_account_info()], signer_seeds)?;
    Ok(())
}

/// Computes the 8-byte Anchor instruction discriminator for a given preimage.
/// Anchor uses SHA-256 (via solana_program::hash) of "global:<instruction_name>".
/// Verify this matches the deployed IDL before going to mainnet.
fn anchor_discriminator(preimage: &[u8]) -> [u8; 8] {
    use solana_program::hash::hash;
    hash(preimage).to_bytes()[..8].try_into().unwrap()
}