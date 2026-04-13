/// Switchboard On-Demand VRF integration.
///
/// Flow
/// ────
/// 1. Agent calls `request_roll(pawn_id)`
///    → our instruction creates a `VRFRequest` PDA
///    → CPIs into Switchboard to open a randomness request,
///      storing the Switchboard `RandomnessAccount` pubkey in `VRFRequest`
///
/// 2. Switchboard oracle network sees the open request on-chain,
///    generates verifiable randomness off-chain, and calls back into
///    our `fulfill_roll` instruction via a CPI signed by the oracle.
///
/// 3. `fulfill_roll` verifies the oracle signature + request linkage,
///    reads the revealed bytes, derives the dice roll, and runs game logic.
///
/// Account layout for Switchboard On-Demand
/// ─────────────────────────────────────────
/// RandomnessAccount  — owned by Switchboard program, one per request.
///   Seeds (Switchboard-managed): [b"Randomness", request_keypair.pubkey()]
///   We pass its pubkey into our VRFRequest so fulfill_roll can verify it.
///
/// Docs: https://switchboard.xyz/docs/on-demand

use anchor_lang::prelude::*;

/// Switchboard On-Demand program ID on mainnet/devnet.
/// Replace with the actual deployed program ID from Switchboard docs.
pub mod switchboard {
    use anchor_lang::declare_id;
    declare_id!("SBondMDrcV3K4kxZR1HNVT7osZxAHVHgYXL5Ze1oMUv");
}

/// Minimal interface for the Switchboard `RandomnessAccount`.
/// We only need `value()` — returns the 32-byte revealed randomness.
/// The full account schema lives in the `switchboard-on-demand` crate;
/// we keep this thin wrapper to avoid pulling the entire dep at the
/// account-context level.
#[derive(Clone)]
pub struct RandomnessAccountData {
    pub result: [u8; 32],
    pub seed:   [u8; 32],
    pub oracle: Pubkey,
}

impl RandomnessAccountData {
    /// Deserialise from a raw `AccountInfo`.
    /// Switchboard encodes: 8-byte discriminator | 32-byte seed | 32-byte result | 32-byte oracle
    pub fn try_deserialize(data: &[u8]) -> Result<Self> {
        require!(data.len() >= 8 + 32 + 32 + 32, crate::errors::LudoError::InvalidVrfAccount);
        let seed:   [u8; 32] = data[8..40].try_into().unwrap();
        let result: [u8; 32] = data[40..72].try_into().unwrap();
        let oracle: [u8; 32] = data[72..104].try_into().unwrap();
        Ok(Self { seed, result, oracle: Pubkey::from(oracle) })
    }

    /// Expand the 32-byte result to 64 bytes (as expected by fulfill_roll).
    /// We hash the result with a counter to fill both halves.
    pub fn expand(&self) -> [u8; 64] {
        use solana_program::keccak;
        let mut out = [0u8; 64];
        let h0 = keccak::hash(&[self.result.as_ref(), b"0"].concat());
        let h1 = keccak::hash(&[self.result.as_ref(), b"1"].concat());
        out[..32].copy_from_slice(&h0.to_bytes());
        out[32..].copy_from_slice(&h1.to_bytes());
        out
    }
}

/// Parameters passed to Switchboard's `request_randomness` CPI.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RequestRandomnessParams {
    /// Commitment identifying this request — we use our `request_id`.
    pub seed: [u8; 32],
    /// Callback instruction that Switchboard will invoke when fulfilled.
    /// Points to our `fulfill_roll` instruction discriminator.
    pub callback: Option<SbCallback>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SbCallback {
    pub program_id:  Pubkey,
    pub accounts:    Vec<SbAccountMeta>,
    pub ix_data:     Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SbAccountMeta {
    pub pubkey:      Pubkey,
    pub is_signer:   bool,
    pub is_writable: bool,
}

/// Build the callback that Switchboard will CPI into once randomness is ready.
/// `fulfill_roll` discriminator = first 8 bytes of sha256("global:fulfill_roll").
pub fn build_fulfill_callback(
    game_engine_program: Pubkey,
    game_state: Pubkey,
    vrf_request: Pubkey,
    crank_fee_payer: Pubkey,
    system_program: Pubkey,
    request_id: [u8; 32],
) -> SbCallback {
    use solana_program::hash::hash;

    // Anchor discriminator = first 8 bytes of SHA256("global:<ix_name>")
    let preimage = b"global:fulfill_roll";
    let disc = &hash(preimage).to_bytes()[..8];

    // Encode instruction data: discriminator + request_id (will be filled by SB oracle)
    // The actual randomness bytes are passed as remaining_accounts by Switchboard.
    let mut ix_data = disc.to_vec();
    ix_data.extend_from_slice(&request_id);
    // randomness [u8; 64] appended by Switchboard oracle at fulfillment time

    SbCallback {
        program_id: game_engine_program,
        accounts: vec![
            SbAccountMeta { pubkey: game_state,     is_signer: false, is_writable: true  },
            SbAccountMeta { pubkey: vrf_request,    is_signer: false, is_writable: true  },
            SbAccountMeta { pubkey: crank_fee_payer,is_signer: true,  is_writable: true  },
            SbAccountMeta { pubkey: system_program, is_signer: false, is_writable: false },
        ],
        ix_data,
    }
}

/// Verify that a `RandomnessAccountData` was produced in response to our
/// specific request. Checks the seed matches our stored `request_id`.
pub fn verify_randomness(
    vrf_data: &RandomnessAccountData,
    expected_seed: &[u8; 32],
) -> Result<()> {
    require!(
        &vrf_data.seed == expected_seed,
        crate::errors::LudoError::RequestIdMismatch
    );
    Ok(())
}
