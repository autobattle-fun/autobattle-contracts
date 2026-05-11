use anchor_lang::prelude::*;

#[error_code]
pub enum MarketError {
    #[msg("Market has already been resolved")]
    MarketAlreadyResolved,

    #[msg("Market has not been resolved yet")]
    MarketNotResolved,

    #[msg("Market has expired")]
    MarketExpired,

    #[msg("Grace period has not elapsed yet (2hr after expiry)")]
    GracePeriodNotOver,

    #[msg("Slippage tolerance exceeded")]
    SlippageExceeded,

    #[msg("Insufficient shares to sell")]
    InsufficientShares,

    #[msg("Position already claimed")]
    AlreadyClaimed,

    #[msg("No winning shares in this position")]
    NoWinningShares,

    #[msg("Arithmetic overflow")]
    Overflow,

    #[msg("Question string exceeds 128 characters")]
    QuestionTooLong,

    #[msg("Signer is not the position owner")]
    UnauthorizedUser,

    #[msg("Claims still pending — vault cannot be closed")]
    ClaimsPending,

    #[msg("Amount must be greater than zero")]
    ZeroAmount,

    #[msg("Bet size exceeds the mathematical limits of the AMM")]
    BetTooLarge,

    #[msg("LP has already been withdrawn")]
    LpAlreadyWithdrawn,

    #[msg("Invalid mint account")]
    InvalidMint,

    #[msg("Market index must be less than 250.")]
    InvalidMarketIndex,

    #[msg("Trade amount is too small, resulting in zero fees.")]
    TradeTooSmall,

    #[msg("LP must withdraw before users can claim.")]
    LpNotYetWithdrawn,

    #[msg("Claim window has expired — use sweep_unclaimed.")]
    ClaimWindowExpired,
    
    #[msg("Claim window not yet over — wait 48 hours after resolution.")]
    ClaimWindowNotOver,
}
