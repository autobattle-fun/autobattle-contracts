use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;

use crate::{
    constants::*,
    errors::LudoError,
    events::*,
    state::*,
    vrf::{self, RandomnessAccountData, RequestRandomnessParams, build_fulfill_callback},
};

// ─────────────────────────────────────────────────────────────────────────────
// init_game
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(
    agent_red: Pubkey,
    agent_blue: Pubkey,
    agent_yellow: Pubkey,
    agent_green: Pubkey,
)]
pub struct InitGame<'info> {
    #[account(
        mut,
        seeds = [REGISTRY_SEED],
        bump  = registry.bump,
    )]
    pub registry: Account<'info, Registry>,

    #[account(
        init,
        seeds = [GAME_SEED, &(registry.game_count + 1).to_le_bytes()],
        bump,
        payer  = crank,
        space  = GameState::LEN,
    )]
    pub game_state: Account<'info, GameState>,

    /// Anyone can crank a new game. Clock check (see handler) prevents early firing.
    #[account(mut)]
    pub crank: Signer<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts[0] = previous GameState PDA (required when game_count > 0)
}

pub fn init_game<'info>(
    ctx: Context<'_, '_, 'info, 'info, InitGame<'info>>,
    agent_red: Pubkey,
    agent_blue: Pubkey,
    agent_yellow: Pubkey,
    agent_green: Pubkey,
) -> Result<()> {
    let reg   = &mut ctx.accounts.registry;
    let clock = Clock::get()?;

    // ── Cooldown guard ────────────────────────────────────────────────────────
    if reg.game_count > 0 {
        let prev_info = ctx.remaining_accounts
            .get(0)
            .ok_or(LudoError::MissingPrevGameState)?;

        let prev_gs: Account<GameState> = Account::try_from(prev_info)?;

        let (expected_pda, _) = Pubkey::find_program_address(
            &[GAME_SEED, &reg.current_game_id.to_le_bytes()],
            ctx.program_id,
        );
        require!(prev_info.key() == expected_pda, LudoError::InvalidPrevGameState);
        require!(
            clock.unix_timestamp >= prev_gs.next_game_starts_at,
            LudoError::CooldownNotOver
        );
    }

    let new_game_id = reg.game_count.checked_add(1).ok_or(LudoError::Overflow)?;
    reg.game_count      = new_game_id;
    reg.current_game_id = new_game_id;
    reg.game_active     = true;

    let gs = &mut ctx.accounts.game_state;
    gs.game_id              = new_game_id;
    gs.agents               = [agent_red, agent_blue, agent_yellow, agent_green];
    gs.phase                = GamePhase::AwaitingRoll;
    gs.active_player        = Color::Red;
    gs.turn_number          = 0;
    gs.consecutive_no_moves = 0;
    gs.pending_request_id   = [0u8; 32];
    gs.pending_pawn_id      = PawnId::new(Color::Red, 0);
    gs.pawn_positions       = [STARTING_SQUARE; 16];
    gs.home_counts          = [0u8; 4];
    gs.winner               = None;
    gs.created_at           = clock.unix_timestamp;
    gs.ended_at             = 0;
    gs.next_game_starts_at  = 0;
    gs.upgrade_locked       = true;
    gs.bump                 = ctx.bumps.game_state;

    emit!(GameInitialised {
        game_id: new_game_id,
        agent_red, agent_blue, agent_yellow, agent_green,
        starts_at: clock.unix_timestamp,
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// request_roll  — Switchboard On-Demand CPI
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(pawn_id: PawnId)]
pub struct RequestRoll<'info> {
    #[account(
        mut,
        seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()],
        bump  = game_state.bump,
    )]
    pub game_state: Account<'info, GameState>,

    /// Short-lived PDA: seeded by (game_id, turn_number) so it's unique per turn.
    #[account(
        init,
        seeds = [VRF_SEED, &game_state.game_id.to_le_bytes(), &game_state.turn_number.to_le_bytes()],
        bump,
        payer = agent,
        space = VRFRequest::LEN,
    )]
    pub vrf_request: Account<'info, VRFRequest>,

    /// Switchboard RandomnessAccount — agent creates this keypair off-chain
    /// and passes it here. Its pubkey becomes our request_id.
    /// CHECK: ownership verified against Switchboard program ID.
    #[account(
        mut,
        owner = vrf::switchboard::ID @ LudoError::InvalidVrfAccount,
    )]
    pub randomness_account: AccountInfo<'info>,

    #[account(mut)]
    pub agent: Signer<'info>,

    /// CHECK: verified by address constraint.
    #[account(address = vrf::switchboard::ID)]
    pub switchboard_program: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn request_roll(ctx: Context<RequestRoll>, pawn_id: PawnId) -> Result<()> {
    let gs    = &mut ctx.accounts.game_state;
    let clock = Clock::get()?;

    require!(gs.phase == GamePhase::AwaitingRoll,    LudoError::NotAwaitingRoll);
    require!(gs.winner.is_none(),                     LudoError::GameAlreadyEnded);
    require!(
        ctx.accounts.agent.key() == gs.agent_for(gs.active_player),
        LudoError::NotYourTurn
    );
    require!(pawn_id.color() == gs.active_player,    LudoError::PawnNotOwnedByPlayer);
    require!(!gs.is_home(pawn_id),                    LudoError::PawnAlreadyHome);

    // request_id = Switchboard RandomnessAccount pubkey (32 bytes)
    let sb_pubkey = ctx.accounts.randomness_account.key();
    let mut request_id = [0u8; 32];
    request_id.copy_from_slice(sb_pubkey.as_ref());

    // Build callback so Switchboard knows which instruction to CPI into
    let callback = build_fulfill_callback(
        *ctx.program_id,
        gs.key(),
        ctx.accounts.vrf_request.key(),
        ctx.accounts.agent.key(),
        ctx.accounts.system_program.key(),
        request_id,
    );

    // ── CPI: Switchboard request_randomness ───────────────────────────────────
    // Discriminator for "request_randomness" on Switchboard On-Demand.
    let ix_disc: [u8; 8] = [0xa0, 0x31, 0x9c, 0x2f, 0x18, 0x76, 0x4f, 0x9c];
    let params = RequestRandomnessParams { seed: request_id, callback: Some(callback) };
    let mut ix_data = ix_disc.to_vec();
    ix_data.extend_from_slice(&params.try_to_vec()?);

    let sb_ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: vrf::switchboard::ID,
        accounts: vec![
            anchor_lang::solana_program::instruction::AccountMeta::new(sb_pubkey, false),
            anchor_lang::solana_program::instruction::AccountMeta::new(ctx.accounts.agent.key(), true),
            anchor_lang::solana_program::instruction::AccountMeta::new_readonly(
                ctx.accounts.system_program.key(), false,
            ),
        ],
        data: ix_data,
    };

    invoke_signed(
        &sb_ix,
        &[
            ctx.accounts.randomness_account.to_account_info(),
            ctx.accounts.agent.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        &[],
    )?;

    // ── Persist ───────────────────────────────────────────────────────────────
    gs.pending_request_id = request_id;
    gs.pending_pawn_id    = pawn_id;
    gs.phase              = GamePhase::AwaitingVRF;

    let vrf          = &mut ctx.accounts.vrf_request;
    vrf.game_id      = gs.game_id;
    vrf.request_id   = request_id;
    vrf.sb_account   = sb_pubkey;
    vrf.pawn_id      = pawn_id;
    vrf.player       = gs.active_player;
    vrf.consumed     = false;
    vrf.requested_at = clock.unix_timestamp;
    vrf.bump         = ctx.bumps.vrf_request;

    emit!(RollRequested {
        game_id: gs.game_id, player: gs.active_player,
        pawn_id, request_id, timestamp: clock.unix_timestamp,
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// fulfill_roll  — invoked by Switchboard oracle as a CPI callback
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(request_id: [u8; 32])]
pub struct FulfillRoll<'info> {
    #[account(
        mut,
        seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()],
        bump  = game_state.bump,
    )]
    pub game_state: Account<'info, GameState>,

    #[account(
        mut,
        seeds = [VRF_SEED, &game_state.game_id.to_le_bytes(), &game_state.turn_number.to_le_bytes()],
        bump  = vrf_request.bump,
        constraint = vrf_request.request_id == request_id @ LudoError::RequestIdMismatch,
        constraint = !vrf_request.consumed               @ LudoError::VrfAlreadyConsumed,
        close  = crank,
    )]
    pub vrf_request: Account<'info, VRFRequest>,

    /// The Switchboard account holding the revealed randomness.
    /// CHECK: owner + pubkey match enforced by constraints.
    #[account(
        owner = vrf::switchboard::ID @ LudoError::InvalidVrfAccount,
        constraint = randomness_account.key() == vrf_request.sb_account @ LudoError::RequestIdMismatch,
    )]
    pub randomness_account: AccountInfo<'info>,

    #[account(mut)]
    pub crank: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn fulfill_roll(
    ctx: Context<FulfillRoll>,
    request_id: [u8; 32],
) -> Result<()> {
    let gs    = &mut ctx.accounts.game_state;
    let clock = Clock::get()?;

    require!(gs.phase == GamePhase::AwaitingVRF, LudoError::NotAwaitingVRF);
    require!(gs.pending_request_id == request_id, LudoError::RequestIdMismatch);

    // ── Read revealed randomness from Switchboard account ─────────────────────
    let sb_data  = ctx.accounts.randomness_account.try_borrow_data()?;
    let vrf_data = RandomnessAccountData::try_deserialize(&sb_data)?;
    vrf::verify_randomness(&vrf_data, &request_id)?;

    let randomness = vrf_data.expand();
    ctx.accounts.vrf_request.consumed = true;

    // ── Dice roll: 1-6 ───────────────────────────────────────────────────────
    let rand_bytes: [u8; 8] = randomness[..8].try_into().unwrap();
    let roll    = (u64::from_le_bytes(rand_bytes) % 6 + 1) as u8;
    let pawn_id = gs.pending_pawn_id;
    let player  = gs.active_player;
    let from_sq = gs.pawn_pos(pawn_id);

    // ── Move ──────────────────────────────────────────────────────────────────
    let maybe_to_sq = compute_move(gs, pawn_id, roll);

    match maybe_to_sq {
        None => {
            gs.consecutive_no_moves += 1;
            emit!(PawnMoved {
                game_id: gs.game_id, player, pawn_id, roll,
                from_square: from_sq, to_square: from_sq,
                captured_pawn: None, turn_number: gs.turn_number,
                timestamp: clock.unix_timestamp,
            });
        }
        Some(to_sq) => {
            gs.consecutive_no_moves = 0;

            if let Some(cap) = check_capture(gs, player, to_sq) {
                gs.set_pawn_pos(cap, STARTING_SQUARE);
                emit!(PawnCaptured {
                    game_id: gs.game_id, capturing_player: player,
                    captured_player: cap.color(), captured_pawn_id: cap,
                    square: to_sq, turn_number: gs.turn_number,
                });
            }

            gs.set_pawn_pos(pawn_id, to_sq);

            if to_sq == HOME_POSITION {
                gs.home_counts[player as usize] += 1;
                emit!(PawnReachedHome {
                    game_id: gs.game_id, player, pawn_id,
                    pawns_home_count: gs.home_counts[player as usize],
                });
            }

            let captured = check_capture(&*gs, player, to_sq); // re-derive for event (already applied above)
            emit!(PawnMoved {
                game_id: gs.game_id, player, pawn_id, roll,
                from_square: from_sq, to_square: to_sq,
                captured_pawn: None, // already emitted PawnCaptured separately
                turn_number: gs.turn_number,
                timestamp: clock.unix_timestamp,
            });
            let _ = captured; // suppress warning
        }
    }

    // ── Win check ─────────────────────────────────────────────────────────────
    if gs.home_counts[player as usize] == PAWNS_PER_PLAYER {
        gs.winner              = Some(player);
        gs.phase               = GamePhase::Ended;
        gs.ended_at            = clock.unix_timestamp;
        gs.next_game_starts_at = clock.unix_timestamp
            .checked_add(DEFAULT_COOLDOWN_SECS)
            .ok_or(LudoError::Overflow)?;

        emit!(GameEnded {
            game_id: gs.game_id, winner: player,
            winner_agent: gs.agent_for(player),
            turn_count: gs.turn_number,
            ended_at: gs.ended_at,
            next_game_starts_at: gs.next_game_starts_at,
        });
        return Ok(());
    }

    // ── Advance turn ──────────────────────────────────────────────────────────
    if roll != 6 || maybe_to_sq.is_none() {
        gs.active_player = next_player(gs.active_player);
    }
    gs.turn_number = gs.turn_number.checked_add(1).ok_or(LudoError::Overflow)?;
    gs.phase       = GamePhase::AwaitingRoll;

    emit!(TurnAdvanced {
        game_id: gs.game_id, turn_number: gs.turn_number,
        active_player: gs.active_player, board_snapshot: gs.snapshot(),
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// end_game  — CPIs into prediction-market to resolve all 4 win markets
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct EndGame<'info> {
    #[account(
        mut,
        seeds = [REGISTRY_SEED],
        bump  = registry.bump,
    )]
    pub registry: Account<'info, Registry>,

    #[account(
        mut,
        seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()],
        bump  = game_state.bump,
        constraint = game_state.game_id == registry.current_game_id @ LudoError::InvalidGameId,
    )]
    pub game_state: Account<'info, GameState>,

    /// CHECK: prediction-market program ID verified via constant.
    #[account(address = PREDICTION_MARKET_PROGRAM_ID)]
    pub prediction_market_program: AccountInfo<'info>,

    pub crank: Signer<'info>,
    pub system_program: Program<'info, System>,

    // remaining_accounts layout:
    // [0] Market PDA for Color::Red   (market_index = 0)
    // [1] Market PDA for Color::Blue  (market_index = 1)
    // [2] Market PDA for Color::Yellow(market_index = 2)
    // [3] Market PDA for Color::Green (market_index = 3)
}

pub fn end_game<'info>(ctx: Context<'_, '_, 'info, 'info, EndGame<'info>>) -> Result<()> {
    let gs = &mut ctx.accounts.game_state;
    require!(gs.phase == GamePhase::Ended, LudoError::GameNotEnded);

    let winner = gs.winner.ok_or(LudoError::GameNotEnded)?;

    let rem = ctx.remaining_accounts;
    require!(rem.len() >= 4, LudoError::MissingMarketAccounts);

    // Anchor discriminator for prediction_market::resolve_market
    let disc = anchor_discriminator(b"global:resolve_market");

    let game_id_bytes  = gs.game_id.to_le_bytes();
    let gs_bump        = gs.bump;
    let signer_seeds: &[&[&[u8]]] = &[&[GAME_SEED, &game_id_bytes, &[gs_bump]]];

    for color_idx in 0u8..4 {
        let color        = Color::from_index(color_idx).unwrap();
        let market_info  = &rem[color_idx as usize];

        // Outcome::Yes(0) for winner, Outcome::No(1) for losers
        let outcome_byte: u8 = if color == winner { 0 } else { 1 };

        let mut ix_data = disc.to_vec();
        ix_data.push(outcome_byte);

        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: PREDICTION_MARKET_PROGRAM_ID,
            accounts: vec![
                anchor_lang::solana_program::instruction::AccountMeta::new(
                    market_info.key(), false,
                ),
                anchor_lang::solana_program::instruction::AccountMeta::new_readonly(
                    ctx.accounts.game_state.key(), false, // PDA authority
                ),
            ],
            data: ix_data,
        };

        invoke_signed(
            &ix,
            &[
                market_info.to_account_info(),
                ctx.accounts.game_state.to_account_info(),
            ],
            signer_seeds,
        )?;
    }

    ctx.accounts.registry.game_active = false;
    // upgrade_locked cleared by prediction-market CPI once claims settle

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// unlock_upgrade  — CPI'd from prediction-market once all claims_remaining == 0
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct UnlockUpgrade<'info> {
    #[account(
        mut,
        seeds = [GAME_SEED, &game_state.game_id.to_le_bytes()],
        bump  = game_state.bump,
    )]
    pub game_state: Account<'info, GameState>,

    /// Only the prediction-market program (as a PDA signer) may call this.
    /// CHECK: address enforced by constant.
    #[account(address = PREDICTION_MARKET_PROGRAM_ID)]
    pub prediction_market_program: Signer<'info>,
}

pub fn unlock_upgrade(ctx: Context<UnlockUpgrade>) -> Result<()> {
    ctx.accounts.game_state.upgrade_locked = false;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Board helpers
// ─────────────────────────────────────────────────────────────────────────────

fn compute_move(gs: &GameState, pawn: PawnId, roll: u8) -> Option<u8> {
    let pos = gs.pawn_pos(pawn);

    if pos == HOME_POSITION   { return None; }
    if pos == STARTING_SQUARE {
        return if roll == 6 { Some(pawn.color().entry_square()) } else { None };
    }

    let entry   = pawn.color().entry_square() as u16;
    let current = pos as u16;
    let track   = BOARD_TRACK_LEN as u16;

    let travelled     = if current >= entry { current - entry } else { track - entry + current };
    let new_travelled = travelled + roll as u16;
    let total_journey = track + HOME_STRETCH_LEN as u16;

    match new_travelled.cmp(&total_journey) {
        std::cmp::Ordering::Greater => None,
        std::cmp::Ordering::Equal   => Some(HOME_POSITION),
        std::cmp::Ordering::Less => {
            if new_travelled >= track {
                Some(52 + (new_travelled - track + 1) as u8) // home stretch: 53-57
            } else {
                Some(((entry + new_travelled) % track) as u8)
            }
        }
    }
}

fn check_capture(gs: &GameState, attacker: Color, to_sq: u8) -> Option<PawnId> {
    if SAFE_SQUARES.contains(&to_sq) || to_sq >= 53 { return None; }
    for ci in 0..4u8 {
        let color = Color::from_index(ci).unwrap();
        if color == attacker { continue; }
        for pi in 0..4u8 {
            let pawn = PawnId::new(color, pi);
            if gs.pawn_pos(pawn) == to_sq { return Some(pawn); }
        }
    }
    None
}

fn next_player(c: Color) -> Color {
    Color::from_index((c as u8 + 1) % 4).unwrap()
}

fn anchor_discriminator(preimage: &[u8]) -> [u8; 8] {
    use solana_program::hash::hash;
    hash(preimage).to_bytes()[..8].try_into().unwrap()
}
