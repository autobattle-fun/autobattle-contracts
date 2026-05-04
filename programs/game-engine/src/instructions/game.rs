use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use switchboard_on_demand::accounts::RandomnessAccountData;

use crate::{constants::*, errors::GameError, events::*, state::*};

// ── Contexts ─────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitGame<'info> {
    #[account(mut, seeds = [REGISTRY_SEED], bump = registry.bump)]
    pub registry: Account<'info, Registry>,
    // Low: Note that (game_count + 1) will overflow the seed array if game_count hits u64::MAX.
    // Given current TPS, this would take millions of years of non-stop game creation.
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
    require!(ctx.accounts.crank.key() == ADMIN_PUBKEY, GameError::UnauthorizedCrank);

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
    gs.pending_commit_slot  = 0;
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
        require!(ctx.accounts.agent.key() == ADMIN_PUBKEY, GameError::UnauthorizedCrank);
    } else if roll_type == 1 {
        require!(gs.phase == GamePhase::AwaitingAction, GameError::InvalidPhase);
        require!(ctx.accounts.agent.key() == gs.agent_for(gs.active_player), GameError::NotYourTurn);
        
        let score = if gs.active_player == Color::Red { gs.p1_score } else { gs.p2_score };
        require!(score <= 21, GameError::Over21CannotHit);

        let already_stayed = if gs.active_player == Color::Red { gs.p1_stayed } else { gs.p2_stayed };
        require!(!already_stayed, GameError::AlreadyStayed);
    } else if roll_type == 2 {
        require!(gs.phase == GamePhase::AwaitingFinalRevealVRF, GameError::InvalidPhase);
        require!(ctx.accounts.agent.key() == ADMIN_PUBKEY, GameError::UnauthorizedCrank);
    } else if roll_type == 3 {
        require!(gs.phase == GamePhase::AwaitingTiebreakerVRF, GameError::InvalidPhase);
        require!(ctx.accounts.agent.key() == ADMIN_PUBKEY, GameError::UnauthorizedCrank);
    } else {
        return err!(GameError::InvalidRollType);
    }

    require!(gs.pending_commit_slot == 0, GameError::VrfAlreadyPending);

    let randomness_data = RandomnessAccountData::parse(
        ctx.accounts.randomness_account.data.borrow()
    ).map_err(|_| GameError::InvalidRandomnessAccount)?;

    require!(randomness_data.seed_slot == clock.slot.saturating_sub(1), GameError::RandomnessExpired);
    require!(randomness_data.get_value(clock.slot).is_err(), GameError::RandomnessAlreadyRevealed);

    gs.pending_commit_slot = randomness_data.seed_slot;

    gs.phase = match roll_type {
        0 => GamePhase::AwaitingInitialDealVRF, 
        1 => GamePhase::AwaitingHitVRF,
        // Fix L2 (Low): Documented self-assignments for final phases
        // Note: phases 2 and 3 remain in their current phase while VRF is pending.
        // Re-entry is blocked by pending_commit_slot != 0, not by phase change.
        2 => GamePhase::AwaitingFinalRevealVRF, 
        _ => GamePhase::AwaitingTiebreakerVRF,  
    };

    let vrf = &mut ctx.accounts.vrf_request;
    vrf.game_id     = gs.game_id;
    vrf.commit_slot = randomness_data.seed_slot;
    vrf.sb_account  = ctx.accounts.randomness_account.key();
    vrf.player      = gs.active_player;
    vrf.roll_type   = roll_type;
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
    require!(ctx.accounts.crank.key() == ADMIN_PUBKEY, GameError::UnauthorizedCrank);

    let clock = Clock::get()?;

    require!(
        ctx.accounts.randomness_account.key() == ctx.accounts.vrf_request.sb_account,
        GameError::InvalidRandomnessAccount
    );

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

    let roll_type = ctx.accounts.vrf_request.roll_type;

    let gs    = &mut ctx.accounts.game_state;
    let inner = &mut **gs;

    if roll_type == 0 {
        inner.p1_last_card = apply_card(&mut inner.p1_score, &mut inner.p1_aces, revealed_random_value[0]);
        inner.p2_last_card = apply_card(&mut inner.p2_score, &mut inner.p2_aces, revealed_random_value[1]);
        inner.phase = GamePhase::AwaitingAction;

        emit!(CardsDealt {
            game_id: inner.game_id,
            p1_score: inner.p1_score,
            p2_score: inner.p2_score,
            is_final_reveal: false,
        });

    } else if roll_type == 1 {
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

        if !(inner.p1_stayed && inner.p2_stayed) {
            if inner.active_player == Color::Red && !inner.p2_stayed {
                inner.active_player = Color::Blue;
            } else if inner.active_player == Color::Blue && !inner.p1_stayed {
                inner.active_player = Color::Red;
            }
        }

        inner.phase = if inner.p1_stayed && inner.p2_stayed {
            GamePhase::AwaitingFinalRevealVRF
        } else {
            GamePhase::AwaitingAction
        };

    } else if roll_type == 2 || roll_type == 3 {
        inner.p1_last_card = apply_card(&mut inner.p1_score, &mut inner.p1_aces, revealed_random_value[0]);
        inner.p2_last_card = apply_card(&mut inner.p2_score, &mut inner.p2_aces, revealed_random_value[1]);
        inner.phase = GamePhase::ReadyToResolve;

        emit!(CardsDealt {
            game_id: inner.game_id,
            p1_score: inner.p1_score,
            p2_score: inner.p2_score,
            is_final_reveal: true,
        });
    } else {
        // Defensive: request_vrf already rejects invalid roll types,
        // so this branch should never be reached in practice.
        return err!(GameError::InvalidRollType);
    }

    inner.pending_commit_slot = 0;

    Ok(())
}

pub fn stay(ctx: Context<Action>, player: Color) -> Result<()> {
    let gs = &mut ctx.accounts.game_state;
    require!(gs.phase == GamePhase::AwaitingAction, GameError::InvalidPhase);
    require!(ctx.accounts.agent.key() == gs.agent_for(player), GameError::NotYourTurn);

    match player {
        Color::Red  => require!(!gs.p1_stayed, GameError::AlreadyStayed),
        Color::Blue => require!(!gs.p2_stayed, GameError::AlreadyStayed),
    }

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
    require!(ctx.accounts.crank.key() == ADMIN_PUBKEY, GameError::UnauthorizedCrank);

    let gs = &mut ctx.accounts.game_state;
    require!(gs.phase == GamePhase::ReadyToResolve, GameError::InvalidPhase);

    // Invariant: round_number is always >= 1 (initialized to 1, incremented with checked_add)
    debug_assert!(gs.round_number > 0);

    let p1_diff = gs.p1_score.abs_diff(21);
    let p2_diff = gs.p2_score.abs_diff(21);

    if p1_diff == p2_diff {
        msg!("Tie detected! Entering Sudden Death.");
        gs.p1_score   = 0;
        gs.p2_score   = 0;
        gs.p1_aces    = 0;
        gs.p2_aces    = 0;
        gs.p1_stayed  = false;
        gs.p2_stayed  = false;
        
        gs.pending_commit_slot = 0; 
        gs.phase = GamePhase::AwaitingTiebreakerVRF;
        
        // Tiebreaker: no round winner, so no round market to resolve.
        // Return early — the round market CPI below does not apply.
        return Ok(());
    }

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

    // Fix M1 (Medium): Gracefully skip round market CPI if the account is uninitialized 
    // or missing, decoupling it from the main market array index requirements.
    if let Some(round_market_info) = ctx.remaining_accounts.get(0) {
        if !round_market_info.data_is_empty() {
            let market_index: u8 = gs.round_number; 
            
            let (expected_pda, _) = Pubkey::find_program_address(
                &[MARKET_SEED, &gs.game_id.to_le_bytes(), &[market_index]],
                &PREDICTION_MARKET_PROGRAM_ID,
            );
            require!(round_market_info.key() == expected_pda, GameError::InvalidMarketAccount);
            
            // gs is passed only for its PDA signing seeds (game_id, bump).
            // The CPI target (prediction market) does not read game_state data.
            resolve_market_cpi(gs, round_market_info, round_winner)?;
        }
    }

    if gs.p1_hp == 0 || gs.p2_hp == 0 {
        let winner_agent = gs.agent_for(round_winner);

        gs.phase  = GamePhase::Ended;
        gs.winner = Some(round_winner);
        ctx.accounts.registry.game_active = false;

        // Fix M1 (cont.): Main market is strictly enforced at index 1 for game-ending logic.
        require!(ctx.remaining_accounts.len() >= 2, GameError::MissingMarketAccounts);
        let main_market_info = &ctx.remaining_accounts[1];

        let (expected_pda, _) = Pubkey::find_program_address(
            &[MARKET_SEED, &gs.game_id.to_le_bytes(), &[0u8]], 
            &PREDICTION_MARKET_PROGRAM_ID,
        );
        require!(main_market_info.key() == expected_pda, GameError::InvalidMarketAccount);
        
        // gs is passed only for its PDA signing seeds (game_id, bump).
        resolve_market_cpi(gs, main_market_info, round_winner)?;

        emit!(GameEnded {
            game_id:      gs.game_id,
            winner:       round_winner,
            winner_agent: winner_agent,
            total_rounds: gs.round_number,
            ended_at:     Clock::get()?.unix_timestamp,
        });

    } else {
        gs.p1_score   = 0;
        gs.p2_score   = 0;
        gs.p1_aces    = 0;
        gs.p2_aces    = 0;
        gs.p1_stayed  = false;
        gs.p2_stayed  = false;
        
        gs.round_number = gs.round_number.checked_add(1).ok_or(GameError::Overflow)?;
        gs.phase = GamePhase::AwaitingInitialDeal;
    }

    Ok(())
}

pub fn unlock_upgrade(ctx: Context<UnlockUpgrade>) -> Result<()> {
    ctx.accounts.game_state.upgrade_locked = false;
    
    emit!(UpgradeUnlocked {
        game_id: ctx.accounts.game_state.game_id,
        timestamp: Clock::get()?.unix_timestamp,
    });
    
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

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

fn resolve_market_cpi<'info>(
    gs: &Account<'info, GameState>,
    market_info: &AccountInfo<'info>,
    winner: Color,
) -> Result<()> {
    let disc = anchor_discriminator(b"global:resolve_market");
    
    // Low: Outcome::Yes = 0 (Red wins), Outcome::No = 1 (Blue wins)
    // This ABI encoding MUST exactly match the Prediction Market IDL structure. 
    // Verify before applying any upgrades to the prediction market.
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

fn anchor_discriminator(preimage: &[u8]) -> [u8; 8] {
    use solana_program::hash::hash;
    hash(preimage).to_bytes()[..8].try_into().expect("sha256 is always 32 bytes")
}