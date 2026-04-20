import * as anchor from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { createMint, createAccount, mintTo, getAccount, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { assert, expect } from "chai";
import BN from "bn.js";

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────
const REGISTRY_SEED = Buffer.from("registry");
const GAME_SEED     = Buffer.from("game");
const VRF_SEED      = Buffer.from("vrf_request");
const MARKET_SEED   = Buffer.from("market");
const VAULT_SEED    = Buffer.from("vault");
const POSITION_SEED = Buffer.from("position");

const AUTO_DECIMALS = 6;
const AUTO_SCALE    = 1_000_000; // 1 $AUTO
const LMSR_B_SCALED = 100 * AUTO_SCALE; // matches LMSR_B_SCALED in rust

// Game Phase Enums (must match Rust)
const GamePhase = {
  AwaitingInitialDeal: { awaitingInitialDeal: {} },
  AwaitingHitVRF: { awaitingHitVrf: {} },
  AwaitingAction: { awaitingAction: {} },
  AwaitingFinalRevealVRF: { awaitingFinalRevealVrf: {} },
  ReadyToResolve: { readyToResolve: {} },
  AwaitingTiebreakerVRF: { awaitingTiebreakerVrf: {} },
  Ended: { ended: {} },
};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────
function gameIdBuf(id: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(id));
  return b;
}

function u8Buf(v: number): Buffer {
  return Buffer.from([v]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test suite
// ─────────────────────────────────────────────────────────────────────────────
describe("autobattle-1v1-blackjack", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  // Load programs (Ensure these match your workspace names)
  const gameEngine = anchor.workspace.GameEngine as anchor.Program;
  const predMarket = anchor.workspace.PredictionMarket as anchor.Program;

  const authority  = (provider.wallet as anchor.Wallet).payer;
  const agentRed   = Keypair.generate();
  const agentBlue  = Keypair.generate();
  const user1      = Keypair.generate();
  const user2      = Keypair.generate();

  let autoMint: PublicKey;
  let adminTokenAccount: PublicKey;
  let user1TokenAccount: PublicKey;
  let user2TokenAccount: PublicKey;

  let [registryPda] = PublicKey.findProgramAddressSync([REGISTRY_SEED], gameEngine.programId);

  // ── Setup ───────────────────────────────────────────────────────────────
  before(async () => {
    // Airdrop SOL
    const signers = [agentRed, agentBlue, user1, user2];
    await Promise.all(
      signers.map((kp) =>
        provider.connection.requestAirdrop(kp.publicKey, 10 * LAMPORTS_PER_SOL)
          .then((sig) => provider.connection.confirmTransaction(sig))
      )
    );

    // Setup $AUTO Token
    autoMint = await createMint(provider.connection, authority, authority.publicKey, null, AUTO_DECIMALS);
    
    adminTokenAccount = await createAccount(provider.connection, authority, autoMint, authority.publicKey);
    user1TokenAccount = await createAccount(provider.connection, authority, autoMint, user1.publicKey);
    user2TokenAccount = await createAccount(provider.connection, authority, autoMint, user2.publicKey);

    // Mint tokens for betting and LP funding
    await mintTo(provider.connection, authority, autoMint, adminTokenAccount, authority, 50000 * AUTO_SCALE);
    await mintTo(provider.connection, authority, autoMint, user1TokenAccount, authority, 10000 * AUTO_SCALE);
    await mintTo(provider.connection, authority, autoMint, user2TokenAccount, authority, 10000 * AUTO_SCALE);
  });

  // ════════════════════════════════════════════════════════════════════════════
  // 1. GAME ENGINE STATE MACHINE
  // ════════════════════════════════════════════════════════════════════════════
  describe("Game Engine: Initialization", () => {
    it("initializes registry", async () => {
      await gameEngine.methods
        .initializeRegistry(new BN(300))
        .accounts({
          registry: registryPda,
          authority: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const reg = await gameEngine.account.registry.fetch(registryPda);
      expect(reg.gameCount.toNumber()).to.eq(0);
    });

    it("init_game creates a 1v1 Deathmatch with 10 HP", async () => {
      const [gamePda] = PublicKey.findProgramAddressSync([GAME_SEED, gameIdBuf(1)], gameEngine.programId);

      await gameEngine.methods
        .initGame(agentRed.publicKey, agentBlue.publicKey)
        .accounts({
          registry: registryPda,
          gameState: gamePda,
          crank: authority.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const gs = await gameEngine.account.gameState.fetch(gamePda);
      expect(gs.gameId.toNumber()).to.eq(1);
      expect(gs.p1Hp).to.eq(10);
      expect(gs.p2Hp).to.eq(10);
      expect(gs.roundNumber).to.eq(1);
      // Ensure phase is strictly set to wait for the first two cards
      expect(gs.phase).to.deep.equal(GamePhase.AwaitingInitialDeal);
    });
  });

  // ════════════════════════════════════════════════════════════════════════════
  // 2. PREDICTION MARKET & SECURITY LIMITS
  // ════════════════════════════════════════════════════════════════════════════
  describe("Prediction Market: AMM Math & Guardrails", () => {
    const GAME_ID = 1;
    const MARKET_IDX = 0; // Win Market

    let marketPda: PublicKey;
    let vaultPda: PublicKey;
    let pos1Pda: PublicKey;

    before(async () => {
      [marketPda] = PublicKey.findProgramAddressSync([MARKET_SEED, gameIdBuf(GAME_ID), u8Buf(MARKET_IDX)], predMarket.programId);
      [vaultPda]  = PublicKey.findProgramAddressSync([VAULT_SEED, gameIdBuf(GAME_ID), u8Buf(MARKET_IDX)], predMarket.programId);
      [pos1Pda]   = PublicKey.findProgramAddressSync([POSITION_SEED, marketPda.toBuffer(), user1.publicKey.toBuffer()], predMarket.programId);
    });

    it("Admin creates the market", async () => {
      const expiresAt = Math.floor(Date.now() / 1000) + 3600;

      await predMarket.methods
        .createMarket(new BN(GAME_ID), MARKET_IDX, "Will Red Win?", new BN(expiresAt))
        .accounts({
          market: marketPda, vault: vaultPda, autoMint: autoMint,
          authority: authority.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId, rent: anchor.web3.SYSVAR_RENT_PUBKEY,
        })
        .rpc();

      const m = await predMarket.account.market.fetch(marketPda);
      expect(m.resolved).to.be.false;
      expect(m.feeBalance.toNumber()).to.eq(0);
    });

    it("Blocks a Whale from breaking the f64 LMSR Math (Over 50x Depth)", async () => {
      // Depth is 100 AUTO. Max bet is 5,000 AUTO. We try to bet 5,001.
      const whaleBet = 5001 * AUTO_SCALE;

      try {
        await predMarket.methods
          .buyShares({ yes: {} }, new BN(whaleBet), new BN(1))
          .accounts({
            market: marketPda, userPosition: pos1Pda, vault: vaultPda,
            userTokenAccount: user1TokenAccount, user: user1.publicKey,
            tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId,
          })
          .signers([user1])
          .rpc();
        assert.fail("Should have thrown BetTooLarge");
      } catch (e: any) {
        expect(e.message).to.include("BetTooLarge");
      }
    });

    it("User buys YES shares and protocol takes exactly 1% fee", async () => {
      const amountIn = 100 * AUTO_SCALE; // 100 AUTO bet
      const expectedFee = 1 * AUTO_SCALE; // 1% is 1 AUTO

      await predMarket.methods
        .buyShares({ yes: {} }, new BN(amountIn), new BN(1))
        .accounts({
          market: marketPda, userPosition: pos1Pda, vault: vaultPda,
          userTokenAccount: user1TokenAccount, user: user1.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();

      const m = await predMarket.account.market.fetch(marketPda);
      expect(m.feeBalance.toNumber()).to.eq(expectedFee); // Vault skimmed the 1%
      
      const vaultAcc = await getAccount(provider.connection, vaultPda);
      expect(Number(vaultAcc.amount)).to.eq(amountIn); // Vault holds the total amount
    });
  });

  // ════════════════════════════════════════════════════════════════════════════
  // 3. ADMIN GARBAGE COLLECTION
  // ════════════════════════════════════════════════════════════════════════════
  describe("Admin LP Withdrawal (Sweep)", () => {
    const GAME_ID = 1;
    const MARKET_IDX = 0; 

    let marketPda: PublicKey;
    let vaultPda: PublicKey;
    let pos1Pda: PublicKey;

    before(async () => {
      [marketPda] = PublicKey.findProgramAddressSync([MARKET_SEED, gameIdBuf(GAME_ID), u8Buf(MARKET_IDX)], predMarket.programId);
      [vaultPda]  = PublicKey.findProgramAddressSync([VAULT_SEED, gameIdBuf(GAME_ID), u8Buf(MARKET_IDX)], predMarket.programId);
      [pos1Pda]   = PublicKey.findProgramAddressSync([POSITION_SEED, marketPda.toBuffer(), user1.publicKey.toBuffer()], predMarket.programId);
    });

    it("Blocks LP withdrawal before the market is resolved", async () => {
      try {
        await predMarket.methods
          .withdrawLp()
          .accounts({
            market: marketPda, vault: vaultPda, adminTokenAccount: adminTokenAccount,
            authority: authority.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
          })
          .rpc();
        assert.fail("Should throw MarketNotResolved");
      } catch (e: any) {
        expect(e.message).to.include("MarketNotResolved");
      }
    });

    it("Blocks LP withdrawal while user claims are still pending", async () => {
      // Force resolve the market
      await predMarket.methods
        .resolveMarket({ yes: {} })
        .accounts({ market: marketPda, authority: authority.publicKey })
        .rpc();

      try {
        await predMarket.methods
          .withdrawLp()
          .accounts({
            market: marketPda, vault: vaultPda, adminTokenAccount: adminTokenAccount,
            authority: authority.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
          })
          .rpc();
        assert.fail("Should throw ClaimsPending");
      } catch (e: any) {
        expect(e.message).to.include("ClaimsPending");
      }
    });

    it("Admin successfully sweeps the vault after users claim winnings", async () => {
      const [gamePda] = PublicKey.findProgramAddressSync([GAME_SEED, gameIdBuf(GAME_ID)], gameEngine.programId);

      // User 1 claims their winnings, dropping claims_remaining to 0
      await predMarket.methods
        .claimPayout()
        .accounts({
          market: marketPda, userPosition: pos1Pda, vault: vaultPda,
          userTokenAccount: user1TokenAccount, user: user1.publicKey,
          // DELETED gameState and gameEngineProgram here!
          tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();
        
      const adminBalBefore = Number((await getAccount(provider.connection, adminTokenAccount)).amount);

      // Admin executes the final sweep
      await predMarket.methods
        .withdrawLp()
        .accounts({
          market: marketPda, vault: vaultPda, adminTokenAccount: adminTokenAccount,
          authority: authority.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();

      const adminBalAfter = Number((await getAccount(provider.connection, adminTokenAccount)).amount);
      
      // Admin should have received their initial LP back + the accumulated 1% fees
      expect(adminBalAfter).to.be.gt(adminBalBefore);

      // Market PDA should be burned and inaccessible
      try {
        await predMarket.account.market.fetch(marketPda);
        assert.fail("Market PDA should be closed");
      } catch (e: any) {
        expect(e.message).to.include("Account does not exist");
      }
    });
  });
});