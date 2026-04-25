use anchor_lang::prelude::*;
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

/// The wallet used by the Deforge AI Backend to resolve mid-game proposals
pub const ADMIN_PUBKEY: Pubkey = solana_program::pubkey!("G2eWnQNwc1wrrgE78NcjmLBXXT9h2s9iUwAM1C8kpFzK");

pub const GAME_ENGINE_PROGRAM_ID: Pubkey = solana_program::pubkey!(
    "GxLT8QMUw6cVT6HQBu2c2zepbQnhUWr4VPEB2vfggE2e"
);

pub const LMSR_B_SCALED: u64 = 14_427_000_000;

/// Grace period before a stuck unresolved market can be refunded (2 hours).
pub const REFUND_GRACE_SECS: i64 = 7_200;

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
        require!(ctx.accounts.authority.key() == ADMIN_PUBKEY, MarketError::UnauthorizedUser);
        require!(question.len() <= 128, MarketError::QuestionTooLong);
        require!(expires_at > clock.unix_timestamp, MarketError::MarketExpired);

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
        
        // 10,000 tokens * 1,000,000 decimals
        let initial_liquidity: u64 = 10_000_000_000;
        
        token::transfer(
            CpiContext::new(cpi_program, cpi_accounts), 
            initial_liquidity
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

        require!(!m.resolved,                          MarketError::MarketAlreadyResolved);
        require!(clock.unix_timestamp < m.expires_at,  MarketError::MarketExpired);
        require!(amount_in > 0,                        MarketError::ZeroAmount);

        let max_allowed_bet = LMSR_B_SCALED.checked_mul(50).ok_or(MarketError::Overflow)?;
        require!(amount_in <= max_allowed_bet, MarketError::BetTooLarge);

        let fee = amount_in.checked_div(100).ok_or(MarketError::Overflow)?;
        let trade_amount = amount_in.checked_sub(fee).ok_or(MarketError::Overflow)?;

        let shares_out = lmsr::calc_shares_out(
            m.yes_supply, m.no_supply, LMSR_B_SCALED, outcome, trade_amount,
        )?;
        require!(shares_out >= min_shares_out, MarketError::SlippageExceeded);

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
            Outcome::Yes => m.yes_supply = m.yes_supply.checked_add(shares_out).ok_or(MarketError::Overflow)?,
            Outcome::No  => m.no_supply  = m.no_supply.checked_add(shares_out).ok_or(MarketError::Overflow)?,
        }
        m.total_volume = m.total_volume.checked_add(trade_amount).ok_or(MarketError::Overflow)?;
        m.fee_balance  = m.fee_balance.checked_add(fee).ok_or(MarketError::Overflow)?;

        let pos = &mut ctx.accounts.user_position;
        if pos.user == Pubkey::default() {
            pos.user         = ctx.accounts.user.key();
            pos.game_id      = m.game_id;
            pos.market_index = m.market_index;
            pos.yes_shares   = 0;
            pos.no_shares    = 0;
            pos.claimed      = false;
            pos.bump         = ctx.bumps.user_position;
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

    pub fn sell_shares(
        ctx: Context<SellShares>,
        outcome: Outcome,
        shares_in: u64,
        min_amount_out: u64,
    ) -> Result<()> {
        let m   = &mut ctx.accounts.market;
        let pos = &mut ctx.accounts.user_position;

        let clock = Clock::get()?;
        require!(clock.unix_timestamp < m.expires_at, MarketError::MarketExpired);

        require!(!m.resolved, MarketError::MarketAlreadyResolved);
        require!(shares_in > 0, MarketError::ZeroAmount);

        match outcome {
            Outcome::Yes => require!(pos.yes_shares >= shares_in, MarketError::InsufficientShares),
            Outcome::No  => require!(pos.no_shares  >= shares_in, MarketError::InsufficientShares),
        }

        let gross_amount_out = lmsr::calc_amount_out(
            m.yes_supply, m.no_supply, LMSR_B_SCALED, outcome, shares_in,
        )?;
        
        let fee = gross_amount_out.checked_div(100).ok_or(MarketError::Overflow)?;
        let net_amount_out = gross_amount_out.checked_sub(fee).ok_or(MarketError::Overflow)?;
        
        require!(net_amount_out >= min_amount_out, MarketError::SlippageExceeded);

        m.fee_balance = m.fee_balance.checked_add(fee).ok_or(MarketError::Overflow)?;

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

        emit!(SharesSold {
            game_id: m.game_id, 
            market_index: m.market_index,
            user: ctx.accounts.user.key(), 
            outcome, 
            shares_in, 
            amount_out: net_amount_out,
        });

        Ok(())
    }

    // ── Resolution ────────────────────────────────────────────────────────────

    pub fn resolve_market(
        ctx: Context<ResolveMarket>,
        outcome: Outcome,
    ) -> Result<()> {
        let m = &mut ctx.accounts.market;
        require!(!m.resolved, MarketError::MarketAlreadyResolved);

        // CPI verification handled by checking against the game engine PDA OR admin
        // No strict CPI signer validation needed here as long as only authorized keys can call this.
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

   pub fn claim_payout(ctx: Context<ClaimPayout>) -> Result<()> {
        let m   = &ctx.accounts.market;
        let pos = &ctx.accounts.user_position;

        require!(m.resolved,   MarketError::MarketNotResolved);
        require!(!pos.claimed, MarketError::AlreadyClaimed);

        let winning_outcome = m.outcome.ok_or(MarketError::MarketNotResolved)?;
        let user_winning_shares = match winning_outcome {
            Outcome::Yes => pos.yes_shares,
            Outcome::No  => pos.no_shares,
        };
        require!(user_winning_shares > 0, MarketError::NoWinningShares);

        // 1 Winning Share = 1 Base Token (1:1 Payout)
        let payout = user_winning_shares;

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

        ctx.accounts.user_position.claimed = true;

        Ok(())
    }

    pub fn refund_expired(ctx: Context<RefundExpired>) -> Result<()> {
        let m   = &ctx.accounts.market;
        let pos = &ctx.accounts.user_position;
        let clock = Clock::get()?;

        require!(!m.resolved,                                      MarketError::MarketAlreadyResolved);
        require!(clock.unix_timestamp > m.expires_at + REFUND_GRACE_SECS, MarketError::GracePeriodNotOver);
        require!(!pos.claimed,                                     MarketError::AlreadyClaimed);

        let user_shares   = pos.yes_shares + pos.no_shares;
        let total_shares  = m.yes_supply + m.no_supply;
        require!(total_shares > 0, MarketError::ZeroAmount);

        let vault_balance = ctx.accounts.vault.amount.saturating_sub(m.fee_balance);
            
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

        ctx.accounts.user_position.claimed = true;

        Ok(())
    }

    pub fn withdraw_lp(ctx: Context<WithdrawLP>) -> Result<()> {
        let market = &mut ctx.accounts.market;
        require!(!market.lp_withdrawn, MarketError::LpAlreadyWithdrawn);

        let vault_balance = ctx.accounts.vault.amount;
        let winning_outcome = market.outcome.ok_or(MarketError::MarketNotResolved)?;

        let total_winning_shares = match winning_outcome {
            Outcome::Yes => market.yes_supply,
            Outcome::No  => market.no_supply,
        };

        // Admin gets the vault balance MINUS the liability owed to winners
        let withdrawable_amount = vault_balance.checked_sub(total_winning_shares).ok_or(MarketError::Overflow)?;

        if withdrawable_amount > 0 {
            let game_id_bytes = market.game_id.to_le_bytes();
            let market_idx = [market.market_index];
            let vault_bump = [market.vault_bump];
            let vault_seeds: &[&[u8]] = &[VAULT_SEED, &game_id_bytes, &market_idx, &vault_bump];

            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.vault.to_account_info(),
                        to: ctx.accounts.admin_token_account.to_account_info(),
                        authority: ctx.accounts.vault.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                withdrawable_amount,
            )?;
        }

        // Reset fee balance to 0 since we just withdrew them alongside the LP
        market.fee_balance = 0; 
        market.lp_withdrawn = true;
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
        constraint = user_token_account.mint == vault.mint @ MarketError::InvalidMint,
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
        constraint = user_position.user == user.key() @ MarketError::UnauthorizedUser,
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
        constraint = user_token_account.mint == vault.mint @ MarketError::InvalidMint,
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
        mut,
        constraint = {
            let (game_pda, _) = Pubkey::find_program_address(
                &[b"game", &market.game_id.to_le_bytes()],
                &GAME_ENGINE_PROGRAM_ID,
            );
            authority.key() == ADMIN_PUBKEY || authority.key() == game_pda
        } @ MarketError::UnauthorizedUser
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
        constraint = user_position.user == user.key() @ MarketError::UnauthorizedUser,
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
        constraint = user_token_account.mint == vault.mint @ MarketError::InvalidMint,
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
        constraint = user_position.user == user.key() @ MarketError::UnauthorizedUser,
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
        constraint = user_token_account.mint == vault.mint @ MarketError::InvalidMint,
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
        constraint = market.resolved @ MarketError::MarketNotResolved,
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
        constraint = admin_token_account.mint == vault.mint @ MarketError::InvalidMint,
    )]
    pub admin_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        address = ADMIN_PUBKEY @ MarketError::UnauthorizedUser,
    )]
    pub authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
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

#[event]
pub struct SharesSold {
    pub game_id:      u64,
    pub market_index: u8,
    pub user:         Pubkey,
    pub outcome:      Outcome,
    pub shares_in:    u64,
    pub amount_out:   u64,
}