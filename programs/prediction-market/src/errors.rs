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
}
