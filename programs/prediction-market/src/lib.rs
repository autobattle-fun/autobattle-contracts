use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("H76M7bbm6zwE464xkabF5MWbciwZqK9FmZYf4omaqnQH");

pub mod errors;
pub mod state;
pub mod lmsr;

use state::*;
use crate::errors::MarketError; 

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const MARKET_SEED: &[u8]    = b"market";
pub const VAULT_SEED: &[u8]     = b"vault";
pub const POSITION_SEED: &[u8]  = b"position";

/// The wallet used by the Deforge AI Backend to resolve mid-game proposals
pub const ADMIN_PUBKEY: Pubkey = solana_program::pubkey!("G2eWnQNwc1wrrgE78NcjmLBXXT9h2s9iUwAM1C8kpFzK");

pub const GAME_ENGINE_PROGRAM_ID: Pubkey = solana_program::pubkey!(
    "GxLT8QMUw6cVT6HQBu2c2zepbQnhUWr4VPEB2vfggE2e"
);

pub const LMSR_B_SCALED: u64 = 144_270_000_000;

/// Grace period before a stuck unresolved market can be refunded (3 minutes for demo).
pub const REFUND_GRACE_SECS: i64 = 180;

// Named constant for initial liquidity
pub const INITIAL_LIQUIDITY: u64 = 100_000_000_000;

/// 48 hours after resolution, unclaimed winnings can be swept
pub const CLAIM_WINDOW_SECS: i64 = 172_800; // 48 hours

// ─────────────────────────────────────────────────────────────────────────────
// Program
// ─────────────────────────────────────────────────────────────────────────────

#[program]
pub mod prediction_market {
    use super::*;

    // ── Market creation ───────────────────────────────────────────────────────

    pub fn create_market(
        ctx: Context<CreateMarket>,
        game_id: u64,
        market_index: u8,
        question: String,
        expires_at: i64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        require!(ctx.accounts.authority.key() == ADMIN_PUBKEY, crate::errors::MarketError::UnauthorizedUser);
        require!(question.len() <= 128, crate::errors::MarketError::QuestionTooLong);
        require!(expires_at > clock.unix_timestamp, crate::errors::MarketError::MarketExpired);
        require!(market_index < 250, crate::errors::MarketError::InvalidMarketIndex);

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
        m.fee_balance     = 0; 
        m.bump            = ctx.bumps.market;
        m.vault_bump      = ctx.bumps.vault;
        m.lp_withdrawn    = false;
        m.resolved_at     = 0;
        m.winner_payout_ratio = 0;

        emit!(MarketCreated {
            game_id,
            market_index,
            expires_at,
            timestamp: clock.unix_timestamp,
        });

        let cpi_accounts = Transfer {
            from: ctx.accounts.creator_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        
        token::transfer(
            CpiContext::new(cpi_program, cpi_accounts), 
            INITIAL_LIQUIDITY
        )?;

        Ok(())
    }

    // ── Trading ───────────────────────────────────────────────────────────────

    pub fn buy_shares(
        ctx: Context<BuyShares>,
        outcome: Outcome,
        amount_in: u64,
        min_shares_out: u64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let m     = &mut ctx.accounts.market;

        require!(!m.resolved,                          crate::errors::MarketError::MarketAlreadyResolved);
        require!(clock.unix_timestamp < m.expires_at,  crate::errors::MarketError::MarketExpired);
        require!(amount_in > 0,                        crate::errors::MarketError::ZeroAmount);

        let max_allowed_bet = LMSR_B_SCALED.checked_mul(50).ok_or(crate::errors::MarketError::Overflow)?;
        require!(amount_in <= max_allowed_bet, crate::errors::MarketError::BetTooLarge);

        let fee = amount_in.checked_div(100).ok_or(crate::errors::MarketError::Overflow)?;
        require!(fee > 0, crate::errors::MarketError::TradeTooSmall);
        
        let trade_amount = amount_in.checked_sub(fee).ok_or(crate::errors::MarketError::Overflow)?;

        let shares_out = lmsr::calc_shares_out(
            m.yes_supply, m.no_supply, LMSR_B_SCALED, outcome, trade_amount,
        )?;
        require!(shares_out >= min_shares_out, crate::errors::MarketError::SlippageExceeded);

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

        match outcome {
            Outcome::Yes => m.yes_supply = m.yes_supply.checked_add(shares_out).ok_or(crate::errors::MarketError::Overflow)?,
            Outcome::No  => m.no_supply  = m.no_supply.checked_add(shares_out).ok_or(crate::errors::MarketError::Overflow)?,
        }
        
        m.total_volume = m.total_volume.checked_add(trade_amount).ok_or(crate::errors::MarketError::Overflow)?;
        m.fee_balance  = m.fee_balance.checked_add(fee).ok_or(crate::errors::MarketError::Overflow)?;

        let pos = &mut ctx.accounts.user_position;
        if !pos.is_initialized {
            pos.is_initialized = true;
            pos.user         = ctx.accounts.user.key();
            pos.game_id      = m.game_id;
            pos.market_index = m.market_index;
            pos.yes_shares   = 0;
            pos.no_shares    = 0;
            pos.claimed      = false;
            pos.bump         = ctx.bumps.user_position;
        }
        match outcome {
            Outcome::Yes => pos.yes_shares = pos.yes_shares.checked_add(shares_out).ok_or(crate::errors::MarketError::Overflow)?,
            Outcome::No  => pos.no_shares  = pos.no_shares.checked_add(shares_out).ok_or(crate::errors::MarketError::Overflow)?,
        }

        emit!(SharesBought {
            game_id: m.game_id, 
            market_index: m.market_index,
            user: ctx.accounts.user.key(), 
            outcome, 
            amount_in, 
            shares_out,
            fee, 
        });

        Ok(())
    }

    pub fn sell_shares(
        ctx: Context<SellShares>,
        outcome: Outcome,
        shares_in: u64,
        min_amount_out: u64,
    ) -> Result<()> {
        let m   = &mut ctx.accounts.market;
        let pos = &mut ctx.accounts.user_position;

        let clock = Clock::get()?;
        require!(clock.unix_timestamp < m.expires_at, crate::errors::MarketError::MarketExpired);
        require!(!m.resolved, crate::errors::MarketError::MarketAlreadyResolved);
        require!(shares_in > 0, crate::errors::MarketError::ZeroAmount);

        match outcome {
            Outcome::Yes => require!(pos.yes_shares >= shares_in, crate::errors::MarketError::InsufficientShares),
            Outcome::No  => require!(pos.no_shares  >= shares_in, crate::errors::MarketError::InsufficientShares),
        }

        let gross_amount_out = lmsr::calc_amount_out(
            m.yes_supply, m.no_supply, LMSR_B_SCALED, outcome, shares_in,
        )?;
        
        let fee = gross_amount_out.checked_div(100).ok_or(crate::errors::MarketError::Overflow)?;
        require!(fee > 0, crate::errors::MarketError::TradeTooSmall);
        
        let net_amount_out = gross_amount_out.checked_sub(fee).ok_or(crate::errors::MarketError::Overflow)?;
        require!(net_amount_out >= min_amount_out, crate::errors::MarketError::SlippageExceeded);

        m.fee_balance = m.fee_balance.checked_add(fee).ok_or(crate::errors::MarketError::Overflow)?;

        match outcome {
            Outcome::Yes => {
                m.yes_supply   = m.yes_supply.checked_sub(shares_in).ok_or(crate::errors::MarketError::Overflow)?;
                pos.yes_shares = pos.yes_shares.checked_sub(shares_in).ok_or(crate::errors::MarketError::Overflow)?;
            }
            Outcome::No => {
                m.no_supply   = m.no_supply.checked_sub(shares_in).ok_or(crate::errors::MarketError::Overflow)?;
                pos.no_shares = pos.no_shares.checked_sub(shares_in).ok_or(crate::errors::MarketError::Overflow)?;
            }
        }

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

        emit!(SharesSold {
            game_id: m.game_id, 
            market_index: m.market_index,
            user: ctx.accounts.user.key(), 
            outcome, 
            shares_in, 
            amount_out: net_amount_out,
            fee, 
        });

        Ok(())
    }

    // ── Resolution ────────────────────────────────────────────────────────────

    pub fn resolve_market(
        ctx: Context<ResolveMarket>,
        outcome: Outcome,
    ) -> Result<()> {
        let clock = Clock::get()?; 
        let m = &mut ctx.accounts.market;
        require!(!m.resolved, crate::errors::MarketError::MarketAlreadyResolved);

        m.resolved    = true;
        m.outcome     = Some(outcome);
        m.resolved_at = clock.unix_timestamp;

        emit!(MarketResolved {
            game_id:      m.game_id,
            market_index: m.market_index,
            outcome,
            yes_supply:   m.yes_supply,
            no_supply:    m.no_supply,
            timestamp:    clock.unix_timestamp,
        });

        Ok(())
    }

    // ── Settlement ────────────────────────────────────────────────────────────

    pub fn claim_payout(ctx: Context<ClaimPayout>) -> Result<()> {
        let m   = &ctx.accounts.market;
        let pos = &mut ctx.accounts.user_position;

        require!(m.resolved,        crate::errors::MarketError::MarketNotResolved);
        require!(!pos.claimed,      crate::errors::MarketError::AlreadyClaimed);
        require!(m.lp_withdrawn,    crate::errors::MarketError::LpNotYetWithdrawn);

        let clock = Clock::get()?;
        require!(
            clock.unix_timestamp <= m.resolved_at + CLAIM_WINDOW_SECS,
            crate::errors::MarketError::ClaimWindowExpired
        );

        let winning_outcome = m.outcome.ok_or(crate::errors::MarketError::MarketNotResolved)?;
        let user_winning_shares = match winning_outcome {
            Outcome::Yes => pos.yes_shares,
            Outcome::No  => pos.no_shares,
        };
        require!(user_winning_shares > 0, crate::errors::MarketError::NoWinningShares);

        let payout = (user_winning_shares as u128)
            .checked_mul(m.winner_payout_ratio as u128)
            .ok_or(crate::errors::MarketError::Overflow)?
            .checked_div(1_000_000_000u128)
            .ok_or(crate::errors::MarketError::Overflow)? as u64;

        require!(payout > 0, crate::errors::MarketError::ZeroAmount);
        pos.claimed = true;

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

        Ok(())
    }

    pub fn refund_expired(ctx: Context<RefundExpired>) -> Result<()> {
        let m   = &mut ctx.accounts.market;       
        let pos = &mut ctx.accounts.user_position;
        let clock = Clock::get()?;

        require!(!m.resolved, crate::errors::MarketError::MarketAlreadyResolved);
        require!(clock.unix_timestamp > m.expires_at + REFUND_GRACE_SECS, crate::errors::MarketError::GracePeriodNotOver);
        require!(!pos.claimed, crate::errors::MarketError::AlreadyClaimed);

        let user_yes = pos.yes_shares;
        let user_no  = pos.no_shares;
        let user_shares = user_yes.checked_add(user_no).ok_or(crate::errors::MarketError::Overflow)?;
        
        require!(user_shares > 0, crate::errors::MarketError::ZeroAmount);

        let total_shares  = m.yes_supply.checked_add(m.no_supply).ok_or(crate::errors::MarketError::Overflow)?;
        require!(total_shares > 0, crate::errors::MarketError::ZeroAmount);

        m.fee_balance = 0; 
        let vault_balance = ctx.accounts.vault.amount;
            
        let refund = (user_shares as u128)
            .checked_mul(vault_balance as u128).ok_or(crate::errors::MarketError::Overflow)?
            .checked_div(total_shares as u128).ok_or(crate::errors::MarketError::Overflow)? as u64;

        pos.claimed = true;
        m.yes_supply = m.yes_supply.checked_sub(user_yes).ok_or(crate::errors::MarketError::Overflow)?;
        m.no_supply  = m.no_supply.checked_sub(user_no).ok_or(crate::errors::MarketError::Overflow)?;

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

        Ok(())
    }

    pub fn withdraw_lp(ctx: Context<WithdrawLP>) -> Result<()> {
        let clock = Clock::get()?;
        let market = &mut ctx.accounts.market;

        require!(ctx.accounts.authority.key() == ADMIN_PUBKEY, crate::errors::MarketError::UnauthorizedUser);
        require!(!market.lp_withdrawn, crate::errors::MarketError::LpAlreadyWithdrawn);

        let vault_balance = ctx.accounts.vault.amount;
        let winning_outcome = market.outcome.ok_or(crate::errors::MarketError::MarketNotResolved)?;

        let total_winning_shares = match winning_outcome {
            Outcome::Yes => market.yes_supply,
            Outcome::No  => market.no_supply,
        };

        // ── LP Protection Logic ──────────────────────────────────────
        let lp_recovery = if vault_balance >= INITIAL_LIQUIDITY {
            let surplus = vault_balance - INITIAL_LIQUIDITY;
            if surplus >= total_winning_shares {
                vault_balance - total_winning_shares
            } else {
                INITIAL_LIQUIDITY
            }
        } else {
            vault_balance
        };

        let remaining_for_winners = vault_balance.saturating_sub(lp_recovery);
        market.winner_payout_ratio = if total_winning_shares > 0 {
            (remaining_for_winners as u128)
                .checked_mul(1_000_000_000u128)
                .ok_or(crate::errors::MarketError::Overflow)?
                .checked_div(total_winning_shares as u128)
                .ok_or(crate::errors::MarketError::Overflow)? as u64
        } else {
            0
        };

        market.fee_balance   = 0;
        market.lp_withdrawn  = true;

        if lp_recovery > 0 {
            let game_id_bytes = market.game_id.to_le_bytes();
            let market_idx    = [market.market_index];
            let vault_bump    = [market.vault_bump];
            let vault_seeds: &[&[u8]] = &[VAULT_SEED, &game_id_bytes, &market_idx, &vault_bump];

            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from:      ctx.accounts.vault.to_account_info(),
                        to:        ctx.accounts.admin_token_account.to_account_info(),
                        authority: ctx.accounts.vault.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                lp_recovery,
            )?;
        }

        emit!(LPWithdrawn {
            game_id:      market.game_id,
            market_index: market.market_index,
            amount:       lp_recovery,
            timestamp:    clock.unix_timestamp,
        });

        Ok(())
    }

    pub fn sweep_unclaimed(ctx: Context<SweepUnclaimed>) -> Result<()> {
        let clock  = Clock::get()?;
        let market = &ctx.accounts.market;

        require!(ctx.accounts.authority.key() == ADMIN_PUBKEY, MarketError::UnauthorizedUser);
        require!(market.resolved,     MarketError::MarketNotResolved);
        require!(market.lp_withdrawn, MarketError::LpNotYetWithdrawn);
        require!(
            clock.unix_timestamp > market.resolved_at + CLAIM_WINDOW_SECS,
            MarketError::ClaimWindowNotOver
        );

        let remaining = ctx.accounts.vault.amount;

        let game_id_bytes = market.game_id.to_le_bytes();
        let market_idx    = [market.market_index];
        let vault_bump    = [market.vault_bump];
        let vault_seeds: &[&[u8]] = &[VAULT_SEED, &game_id_bytes, &market_idx, &vault_bump];

        // 1. Transfer remaining tokens to admin
        if remaining > 0 {
            token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.vault.to_account_info(),
                    to:        ctx.accounts.admin_token_account.to_account_info(),
                    authority: ctx.accounts.vault.to_account_info(),
                },
                &[vault_seeds],
            ),
            remaining,
        )?;
    }

        // 2. Close vault token account — returns rent to authority
        token::close_account(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::CloseAccount {
                    account:     ctx.accounts.vault.to_account_info(),
                    destination: ctx.accounts.authority.to_account_info(),
                    authority:   ctx.accounts.vault.to_account_info(),
                },
                &[vault_seeds],
            ),
        )?;

        // 3. Market account closed via `close = authority` in context

        emit!(UnclaimedSwept {
            game_id:      market.game_id,
            market_index: market.market_index,
            amount:       remaining,
            timestamp:    clock.unix_timestamp,
        });
        
        Ok(())
    }
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
    pub creator_token_account: Account<'info, TokenAccount>,

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

    #[account(
        mut,
        constraint = user_token_account.mint == vault.mint @ crate::errors::MarketError::InvalidMint,
    )]
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
        constraint = user_position.user == user.key() @ crate::errors::MarketError::UnauthorizedUser,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_account.mint == vault.mint @ crate::errors::MarketError::InvalidMint,
    )]
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

    #[account(
        constraint = {
            let (game_pda, _) = Pubkey::find_program_address(
                &[b"game", &market.game_id.to_le_bytes()],
                &GAME_ENGINE_PROGRAM_ID,
            );
            authority.key() == ADMIN_PUBKEY || authority.key() == game_pda
        } @ crate::errors::MarketError::UnauthorizedUser
    )]
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
        constraint = user_position.user == user.key() @ crate::errors::MarketError::UnauthorizedUser,
        close = user,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_account.mint == vault.mint @ crate::errors::MarketError::InvalidMint,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,

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
        constraint = user_position.user == user.key() @ crate::errors::MarketError::UnauthorizedUser,
        close = user,
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_account.mint == vault.mint @ crate::errors::MarketError::InvalidMint,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,

    pub token_program:  Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct WithdrawLP<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump = market.bump,
        constraint = market.resolved @ crate::errors::MarketError::MarketNotResolved,
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = admin_token_account.mint == vault.mint @ crate::errors::MarketError::InvalidMint,
    )]
    pub admin_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        address = ADMIN_PUBKEY @ crate::errors::MarketError::UnauthorizedUser,
    )]
    pub authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SweepUnclaimed<'info> {
    #[account(
        mut,
        seeds = [MARKET_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.bump,
        constraint = market.resolved     @ MarketError::MarketNotResolved,
        constraint = market.lp_withdrawn @ MarketError::LpNotYetWithdrawn,
        close = authority,
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.game_id.to_le_bytes(), &[market.market_index]],
        bump  = market.vault_bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = admin_token_account.mint == vault.mint @ MarketError::InvalidMint,
    )]
    pub admin_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        address = ADMIN_PUBKEY @ MarketError::UnauthorizedUser,
    )]
    pub authority: Signer<'info>,

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
    pub fee:          u64, 
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

#[event]
pub struct SharesSold {
    pub game_id:      u64,
    pub market_index: u8,
    pub user:         Pubkey,
    pub outcome:      Outcome,
    pub shares_in:    u64,
    pub amount_out:   u64,
    pub fee:          u64, 
}

#[event]
pub struct LPWithdrawn {
    pub game_id:      u64,
    pub market_index: u8,
    pub amount:       u64,
    pub timestamp:    i64,
}

#[event]
pub struct UnclaimedSwept {
    pub game_id:      u64,
    pub market_index: u8,
    pub amount:       u64,
    pub timestamp:    i64,
}