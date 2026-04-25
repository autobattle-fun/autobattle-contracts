use anchor_lang::prelude::*;

#[error_code]
pub enum GameError {
    #[msg("Cooldown period has not elapsed yet")]
    CooldownNotOver,
    #[msg("Cannot update cooldown while a game is in progress")]
    GameInProgress,
    #[msg("It is not this agent's turn")]
    NotYourTurn,
    #[msg("Signer is not a registered agent for this game")]
    UnauthorizedAgent,
    #[msg("Game is not in the correct phase for this action")]
    InvalidPhase,
    #[msg("Game has already ended")]
    GameAlreadyEnded,
    #[msg("Game has not ended yet")]
    GameNotEnded,
    #[msg("Player score is 21 or over, cannot hit")]
    Over21CannotHit,
    #[msg("Both players must complete the final reveal before resolving the round")]
    RoundNotFinished,
    #[msg("VRF request ID mismatch")]
    RequestIdMismatch,
    #[msg("VRF result has already been consumed")]
    VrfAlreadyConsumed,
    #[msg("Account is not owned by the Switchboard program")]
    InvalidVrfAccount,
    #[msg("Randomness has expired or slot mismatch")]
    RandomnessExpired,
    #[msg("Randomness already revealed")]
    RandomnessAlreadyRevealed,
    #[msg("Randomness not yet resolved by oracle")]
    RandomnessNotResolved,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Upgrade locked: open markets or unclaimed positions exist")]
    UpgradeLocked,
    #[msg("Previous GameState PDA does not match expected address")]
    InvalidPrevGameState,
    #[msg("Game ID does not match registry current_game_id")]
    InvalidGameId,
    #[msg("Account is not a valid Switchboard randomness account")]
    InvalidRandomnessAccount,
    #[msg("Provided market account does not match the expected PDA")]
    InvalidMarketAccount,
    #[msg("Signer is not the authorized crank for this operation")]
    UnauthorizedCrank,
}