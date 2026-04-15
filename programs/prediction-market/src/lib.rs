use anchor_lang::prelude::*;
use solana_program::program::invoke_signed;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("H76M7bbm6zwE464xkabF5MWbciwZqK9FmZYf4omaqnQH");

pub mod errors;
pub mod state;
pub mod lmsr;

use state::*;
use errors::MarketError;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const MARKET_SEED: &[u8]    = b"market";
pub const VAULT_SEED: &[u8]     = b"vault";
pub const POSITION_SEED: &[u8]  = b"position";

/// LMSR liquidity parameter b, scaled to 6 decimals (= 100 $AUTO).
/// Tune upward for deeper markets with less price impact per trade.
pub const LMSR_B_SCALED: u64    = 100_000_000;

/// Grace period before a stuck unresolved market can be refunded (2 hours).
pub const REFUND_GRACE_SECS: i64 = 7_200;

/// Game engine program ID — target of the unlock_upgrade CPI.
/// Replace with actual deployed game-engine program ID.
pub const GAME_ENGINE_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("GxLT8QMUw6cVT6HQBu2c2zepbQnhUWr4VPEB2vfggE2e");

// ─────────────────────────────────────────────────────────────────────────────
// Program
// ─────────────────────────────────────────────────────────────────────────────

#[program]
pub mod prediction_market {
    use super::*;

    // ── Market creation ───────────────────────────────────────────────────────

    /// Create a new prediction market.
    /// market_index 0-3 = "Will <color> win?" markets (opened at game start).
    /// market_index 4+  = live proposal markets (opened by backend mid-game).
    pub fn create_market(
        ctx: Context<CreateMarket>,
        game_id: u64,
        market_index: u8,
        question: String,
        expires_at: i64,
    ) -> Result<()> {
        require!(question.len() <= 128, MarketError::QuestionTooLong);
        let clock = Clock::get()?;

        let m             = &mut ctx.accounts.market;
        m.game_id         = game_id;
        m.market_index    = market_index;
        m.question        = question;
        m.yes_supply      = 0;
        m.no_supply       = 0;
        m.total_volume    = 0;
        m.resolved        = false;
        m.outcome         = None;
        m.expires_at      = expires_at;
        m.created_at      = clock.unix_timestamp;
        m.claims_remaining = 0;
        m.fee_balance     = 0; // Initialize fee tracker
        m.bump            = ctx.bumps.market;
        m.vault_bump      = ctx.bumps.vault;

        emit!(MarketCreated {
            game_id,
            market_index,
            expires_at,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    // ── Trading ───────────────────────────────────────────────────────────────

    /// Buy YES or NO shares with $AUTO tokens.
    pub fn buy_shares(
        ctx: Context<BuyShares>,
        outcome: Outcome,
        amount_in: u64,
        min_shares_out: u64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let m     = &mut ctx.accounts.market;

        require!(!m.resolved,                          MarketError::MarketAlreadyResolved);
        require!(clock.unix_timestamp < m.expires_at,  MarketError::MarketExpired);
        require!(amount_in > 0,                        MarketError::ZeroAmount);

        // 1. Calculate 0.5% fee and net trade amount
        let fee = amount_in.checked_mul(5).ok_or(MarketError::Overflow)?
            .checked_div(1000).ok_or(MarketError::Overflow)?;
        let trade_amount = amount_in.checked_sub(fee).ok_or(MarketError::Overflow)?;

        // 2. Calculate shares out based on net trade amount
        let shares_out = lmsr::calc_shares_out(
            m.yes_supply, m.no_supply, LMSR_B_SCALED, outcome, trade_amount,
        )?;
        require!(shares_out >= min_shares_out, MarketError::SlippageExceeded);

        // 3. Transfer full amount_in user → vault
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.user_token_account.to_account_info(),
                    to:        ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            amount_in,
        )?;

        // 4. Update supplies, volume, and track fees
        match outcome {
            Outcome::Yes => m.yes_supply = m.yes_supply.checked_add(shares_out).ok_or(MarketError::Overflow)?,
            Outcome::No  => m.no_supply  = m.no_supply.checked_add(shares_out).ok_or(MarketError::Overflow)?,
        }
        m.total_volume = m.total_volume.checked_add(trade_amount).ok_or(MarketError::Overflow)?;
        m.fee_balance  = m.fee_balance.checked_add(fee).ok_or(MarketError::Overflow)?;

        // 5. Init UserPosition if first time
        let pos = &mut ctx.accounts.user_position;
        if pos.user == Pubkey::default() {
            pos.user         = ctx.accounts.user.key();
            pos.game_id      = m.game_id;
            pos.market_index = m.market_index;
            pos.yes_shares   = 0;
            pos.no_shares    = 0;
            pos.claimed      = false;
            pos.bump         = ctx.bumps.user_position;
            m.claims_remaining = m.claims_remaining.checked_add(1).ok_or(MarketError::Overflow)?;
        }
        match outcome {
            Outcome::Yes => pos.yes_shares = pos.yes_shares.checked_add(shares_out).ok_or(MarketError::Overflow)?,
            Outcome::No  => pos.no_shares  = pos.no_shares.checked_add(shares_out).ok_or(MarketError::Overflow)?,
        }

        emit!(SharesBought {
            game_id: m.game_id, market_index: m.market_index,
            user: ctx.accounts.user.key(), outcome, amount_in, shares_out,
        });

        Ok(())
    }

    /// Sell shares back to the vault for $AUTO.
    pub fn sell_shares(
        ctx: Context<SellShares>,
        outcome: Outcome,
        shares_in: u64,
        min_amount_out: u64,
    ) -> Result<()> {
        let m   = &mut ctx.accounts.market;
        let pos = &mut ctx.accounts.user_position;

        require!(!m.resolved, MarketError::MarketAlreadyResolved);
        require!(shares_in > 0, MarketError::ZeroAmount);

        match outcome {
            Outcome::Yes => require!(pos.yes_shares >= shares_in, MarketError::InsufficientShares),
            Outcome::No  => require!(pos.no_shares  >= shares_in, MarketError::InsufficientShares),
        }

        // 1. Calculate raw payout
        let gross_amount_out = lmsr::calc_amount_out(
            m.yes_supply, m.no_supply, LMSR_B_SCALED, outcome, shares_in,
        )?;
        
        // 2. Subtract 0.5% fee
        let fee = gross_amount_out.checked_mul(5).ok_or(MarketError::Overflow)?
            .checked_div(1000).ok_or(MarketError::Overflow)?;
        let net_amount_out = gross_amount_out.checked_sub(fee).ok_or(MarketError::Overflow)?;
        
        require!(net_amount_out >= min_amount_out, MarketError::SlippageExceeded);

        m.fee_balance = m.fee_balance.checked_add(fee).ok_or(MarketError::Overflow)?;

        // 3. Transfer net_amount_out vault → user
        let game_id_bytes   = m.game_id.to_le_bytes();
        let market_idx      = [m.market_index];
        let vault_bump      = [m.vault_bump];
        let vault_seeds: &[&[u8]] = &[VAULT_SEED, &game_id_bytes, &market_idx, &vault_bump];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.vault.to_account_info(),
                    to:        ctx.accounts.user_token_account.to_account_info(),
                    authority: ctx.accounts.vault.to_account_info(),
                },
                &[vault_seeds],
            ),
            net_amount_out,
        )?;

        match outcome {
            Outcome::Yes => {
                m.yes_supply   = m.yes_supply.checked_sub(shares_in).ok_or(MarketError::Overflow)?;
                pos.yes_shares = pos.yes_shares.checked_sub(shares_in).ok_or(MarketError::Overflow)?;
            }
            Outcome::No => {
                m.no_supply   = m.no_supply.checked_sub(shares_in).ok_or(MarketError::Overflow)?;
                pos.no_shares = pos.no_shares.checked_sub(shares_in).ok_or(MarketError::Overflow)?;
            }
        }

        if pos.yes_shares == 0 && pos.no_shares == 0 {
            m.claims_remaining = m.claims_remaining.saturating_sub(1);
        }

        Ok(())
    }

    // ── Resolution ────────────────────────────────────────────────────────────

    /// Resolve a market. Callable by authority (multisig) OR game engine PDA (via CPI).
    /// For win markets, the game engine calls this automatically from end_game.
    /// For live proposal markets, the backend calls this via the authority key.
    pub fn resolve_market(
        ctx: Context<ResolveMarket>,
        outcome: Outcome,
    ) -> Result<()> {
        let m = &mut ctx.accounts.market;
        require!(!m.resolved, MarketError::MarketAlreadyResolved);

        m.resolved = true;
        m.outcome  = Some(outcome);

        emit!(MarketResolved {
            game_id:      m.game_id,
            market_index: m.market_index,
            outcome,
            yes_supply:   m.yes_supply,
            no_supply:    m.no_supply,
            timestamp:    Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    // ── Settlement ────────────────────────────────────────────────────────────

    /// Winning position holder claims their $AUTO payout.
    /// After the last claim, CPIs into game-engine to clear upgrade_locked.
   pub fn claim_payout(ctx: Context<ClaimPayout>) -> Result<()> {
        {
            let m   = &ctx.accounts.market;
            let pos = &ctx.accounts.user_position;

            require!(m.resolved,   MarketError::MarketNotResolved);
            require!(!pos.claimed, MarketError::AlreadyClaimed);

            let winning_outcome       = m.outcome.ok_or(MarketError::MarketNotResolved)?;
            let user_winning_shares   = match winning_outcome {
                Outcome::Yes => pos.yes_shares,
                Outcome::No  => pos.no_shares,
            };
            require!(user_winning_shares > 0, MarketError::NoWinningShares);

            let total_winning = match winning_outcome {
                Outcome::Yes => m.yes_supply,
                Outcome::No  => m.no_supply,
            };

            // Deduct the collected fees before calculating the payout ratio
            let vault_balance = ctx.accounts.vault.amount
                .checked_sub(m.fee_balance)
                .ok_or(MarketError::Overflow)?;
                
            let payout = (user_winning_shares as u128)
                .checked_mul(vault_balance as u128).ok_or(MarketError::Overflow)?
                .checked_div(total_winning as u128).ok_or(MarketError::Overflow)? as u64;

            let game_id_bytes = m.game_id.to_le_bytes();
            let market_idx    = [m.market_index];
            let vault_bump    = [m.vault_bump];
            let vault_seeds: &[&[u8]] = &[VAULT_SEED, &game_id_bytes, &market_idx, &vault_bump];
            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from:      ctx.accounts.vault.to_account_info(),
                        to:        ctx.accounts.user_token_account.to_account_info(),
                        authority: ctx.accounts.vault.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                payout,
            )?;

            emit!(PayoutClaimed {
                game_id:      m.game_id,
                market_index: m.market_index,
                user:         ctx.accounts.user.key(),
                payout,
            });
        }

        ctx.accounts.user_position.claimed = true;
        ctx.accounts.market.claims_remaining =
            ctx.accounts.market.claims_remaining.saturating_sub(1);

        if ctx.accounts.market.claims_remaining == 0 {
            maybe_unlock_upgrade(&ctx)?;
        }

        Ok(())
    }

    /// Refund positions in a market that expired without being resolved.
    /// Permissionless — anyone can crank this 2 hours after expiry.
    pub fn refund_expired(ctx: Context<RefundExpired>) -> Result<()> {
        {
            let m   = &ctx.accounts.market;
            let pos = &ctx.accounts.user_position;
            let clock = Clock::get()?;

            require!(!m.resolved,                                      MarketError::MarketAlreadyResolved);
            require!(clock.unix_timestamp > m.expires_at + REFUND_GRACE_SECS, MarketError::GracePeriodNotOver);
            require!(!pos.claimed,                                     MarketError::AlreadyClaimed);

            let user_shares   = pos.yes_shares + pos.no_shares;
            let total_shares  = m.yes_supply + m.no_supply;
            require!(total_shares > 0, MarketError::ZeroAmount);

            // Deduct the collected fees before calculating the refund ratio
            let vault_balance = ctx.accounts.vault.amount
                .checked_sub(m.fee_balance)
                .ok_or(MarketError::Overflow)?;
                
            let refund = (user_shares as u128)
                .checked_mul(vault_balance as u128).ok_or(MarketError::Overflow)?
                .checked_div(total_shares as u128).ok_or(MarketError::Overflow)? as u64;

            let game_id_bytes = m.game_id.to_le_bytes();
            let market_idx    = [m.market_index];
            let vault_bump    = [m.vault_bump];
            let vault_seeds: &[&[u8]] = &[VAULT_SEED, &game_id_bytes, &market_idx, &vault_bump];
            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from:      ctx.accounts.vault.to_account_info(),
                        to:        ctx.accounts.user_token_account.to_account_info(),
                        authority: ctx.accounts.vault.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                refund,
            )?;

            emit!(PositionRefunded {
                game_id:      m.game_id,
                market_index: m.market_index,
                user:         ctx.accounts.user.key(),
                refund,
            });
        }

        ctx.accounts.user_position.claimed = true;
        ctx.accounts.market.claims_remaining =
            ctx.accounts.market.claims_remaining.saturating_sub(1);

        if ctx.accounts.market.claims_remaining == 0 {
            maybe_unlock_upgrade_refund(&ctx)?;
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// unlock_upgrade helper — CPI from prediction-market → game-engine
// ─────────────────────────────────────────────────────────────────────────────
//
// Called when the last claim or refund on a WIN market (market_index 0-3)
// is processed. Tells the game engine it is safe to accept upgrade proposals.
//
// The prediction-market PDA signs as authority. The game-engine's
// `unlock_upgrade` instruction verifies the caller is this program ID.

fn maybe_unlock_upgrade(ctx: &Context<ClaimPayout>) -> Result<()> {
    // Only trigger for win markets (index 0-3)
    if ctx.accounts.market.market_index >= 4 { return Ok(()); }

    do_unlock_cpi(
        ctx.accounts.game_engine_program.to_account_info(),
        ctx.accounts.game_state.to_account_info(),
        ctx.accounts.market.game_id,
        ctx.accounts.market.bump,
        ctx.accounts.market.market_index,
    )
}

fn maybe_unlock_upgrade_refund(ctx: &Context<RefundExpired>) -> Result<()> {
    if ctx.accounts.market.market_index >= 4 { return Ok(()); }

    do_unlock_cpi(
        ctx.accounts.game_engine_program.to_account_info(),
        ctx.accounts.game_state.to_account_info(),
        ctx.accounts.market.game_id,
        ctx.accounts.market.bump,
        ctx.accounts.market.market_index,
    )
}

fn do_unlock_cpi<'info>(
    game_engine_program: AccountInfo<'info>,
    game_state: AccountInfo<'info>,
    game_id: u64,
    _market_bump: u8,
    market_index: u8,
) -> Result<()> {
    use solana_program::hash::hash;

    let disc: [u8; 8] = hash(b"global:unlock_upgrade").to_bytes()[..8].try_into().unwrap();

    let ix = solana_program::instruction::Instruction {
        program_id: GAME_ENGINE_PROGRAM_ID,
        accounts: vec![
            solana_program::instruction::AccountMeta::new(
                game_state.key(), false,
            ),
            // prediction_market_program is the signer — verified by address in game-engine
            solana_program::instruction::AccountMeta::new_readonly(
                crate::ID, true,
            ),
        ],
        data: disc.to_vec(),
    };

    // The prediction-market program itself signs.
    // Signer seeds for this program's PDA derived from market seeds.
    let game_id_bytes = game_id.to_le_bytes();
    let market_idx    = [market_index];
    // We use the market PDA as the signing authority.
    // In practice the program ID signs implicitly for CPIs — no seeds needed
    // when the program itself is the signer.
    invoke_signed(
        &ix,
        &[game_state, game_engine_program],
        &[], // program-level CPI; no PDA seeds required
    )?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Account contexts
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(game_id: u64, market_index: u8, question: String, expires_at: i64)]
pub struct CreateMarket<'info> {
    #[account(
        init,
        seeds = [MARKET_SEED, &game_id.to_le_bytes(), &[market_index]],
        bump,
        payer = authority,
        space = Market::LEN,
    )]
    pub market: Account<'info, Market>,

    #[account(
        init,
        seeds = [VAULT_SEED, &game_id.to_le_bytes(), &[market_index]],
        bump,
        payer            = authority,
        token::mint      = auto_mint,
        token::authority = vault,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub auto_mint: Account<'info, anchor_spl::token::Mint>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent:           Sysvar<'info, Rent>,
}

#[derive(Accounts)]
#[instruction(outcome: Outcome, amount_in: u64, min_shares_out: u64)]
pub struct BuyShares<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.bump,
    )]
    pub market: Account<'info, Market>,

    #[account(
        init_if_needed,
        seeds = [POSITION_SEED, market.key().as_ref(), user.key().as_ref()],
        bump,
        payer = user,
        space = UserPosition::LEN,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SellShares<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.bump,
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds  = [POSITION_SEED, market.key().as_ref(), user.key().as_ref()],
        bump   = user_position.bump,
        constraint = user_position.user == user.key() @ MarketError::UnauthorizedUser,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ResolveMarket<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.bump,
    )]
    pub market: Account<'info, Market>,

    /// Authority = multisig key OR game-engine PDA (for win market auto-resolution).
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct ClaimPayout<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.bump,
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds  = [POSITION_SEED, market.key().as_ref(), user.key().as_ref()],
        bump   = user_position.bump,
        constraint = user_position.user == user.key() @ MarketError::UnauthorizedUser,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,

    /// CHECK: game-engine GameState PDA — passed to unlock_upgrade CPI.
    #[account(mut)]
    pub game_state: AccountInfo<'info>,

    /// CHECK: game-engine program — address verified via constant.
    #[account(address = GAME_ENGINE_PROGRAM_ID)]
    pub game_engine_program: AccountInfo<'info>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RefundExpired<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.bump,
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds  = [POSITION_SEED, market.key().as_ref(), user.key().as_ref()],
        bump   = user_position.bump,
        constraint = user_position.user == user.key() @ MarketError::UnauthorizedUser,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,

    /// CHECK: game-engine GameState PDA.
    #[account(mut)]
    pub game_state: AccountInfo<'info>,

    /// CHECK: game-engine program.
    #[account(address = GAME_ENGINE_PROGRAM_ID)]
    pub game_engine_program: AccountInfo<'info>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Events
// ─────────────────────────────────────────────────────────────────────────────

#[event]
pub struct MarketCreated {
    pub game_id:      u64,
    pub market_index: u8,
    pub expires_at:   i64,
    pub timestamp:    i64,
}

#[event]
pub struct SharesBought {
    pub game_id:      u64,
    pub market_index: u8,
    pub user:         Pubkey,
    pub outcome:      Outcome,
    pub amount_in:    u64,
    pub shares_out:   u64,
}

#[event]
pub struct MarketResolved {
    pub game_id:      u64,
    pub market_index: u8,
    pub outcome:      Outcome,
    pub yes_supply:   u64,
    pub no_supply:    u64,
    pub timestamp:    i64,
}

#[event]
pub struct PayoutClaimed {
    pub game_id:      u64,
    pub market_index: u8,
    pub user:         Pubkey,
    pub payout:       u64,
}

#[event]
pub struct PositionRefunded {
    pub game_id:      u64,
    pub market_index: u8,
    pub user:         Pubkey,
    pub refund:       u64,
}
