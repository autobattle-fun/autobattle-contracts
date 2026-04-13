use anchor_lang::prelude::*;

#[error_code]
pub enum LudoError {
    // ── Registry ──────────────────────────────────────────────────────────────
    #[msg("Cooldown period has not elapsed yet")]
    CooldownNotOver,

    #[msg("Cannot update cooldown while a game is in progress")]
    GameInProgress,

    // ── Turn & agent validation ───────────────────────────────────────────────
    #[msg("It is not this agent's turn")]
    NotYourTurn,

    #[msg("Signer is not a registered agent for this game")]
    UnauthorizedAgent,

    // ── Phase guards ──────────────────────────────────────────────────────────
    #[msg("Game is not in the AwaitingRoll phase")]
    NotAwaitingRoll,

    #[msg("Game is not in the AwaitingVRF phase")]
    NotAwaitingVRF,

    #[msg("Game has already ended")]
    GameAlreadyEnded,

    #[msg("Game has not ended yet")]
    GameNotEnded,

    // ── Pawn validation ───────────────────────────────────────────────────────
    #[msg("Pawn does not belong to the active player")]
    PawnNotOwnedByPlayer,

    #[msg("Pawn is already home")]
    PawnAlreadyHome,

    #[msg("Cannot move: pawn is in yard and roll is not 6")]
    PawnInYardRollNotSix,

    #[msg("No valid pawn move available for this roll")]
    NoValidMove,

    // ── VRF ───────────────────────────────────────────────────────────────────
    #[msg("VRF request ID mismatch")]
    RequestIdMismatch,

    #[msg("VRF result has already been consumed")]
    VrfAlreadyConsumed,

    // ── Misc ──────────────────────────────────────────────────────────────────
    #[msg("Arithmetic overflow")]
    Overflow,

    #[msg("Invalid pawn ID string")]
    InvalidPawnId,

    #[msg("Upgrade locked: open markets or unclaimed positions exist")]
    UpgradeLocked,

    #[msg("Previous GameState account must be passed in remaining_accounts[0]")]
    MissingPrevGameState,

    #[msg("Previous GameState PDA does not match expected address")]
    InvalidPrevGameState,

    #[msg("Game ID does not match registry current_game_id")]
    InvalidGameId,

    #[msg("At least 4 market accounts must be passed in remaining_accounts")]
    MissingMarketAccounts,

    #[msg("Account is not owned by the Switchboard program")]
    InvalidVrfAccount,
}
