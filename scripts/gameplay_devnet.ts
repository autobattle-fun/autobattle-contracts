import * as anchor from "@coral-xyz/anchor";
const { BN, web3 } = anchor;
import { Keypair, PublicKey, SystemProgram, ComputeBudgetProgram } from "@solana/web3.js";
import { getOrCreateAssociatedTokenAccount, TOKEN_PROGRAM_ID, mintTo, getAccount } from "@solana/spl-token";
import * as sb from "@switchboard-xyz/on-demand";

// --- Helpers ---
function gameIdBuf(id: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(id));
  return b;
}
function u8Buf(v: number): Buffer {
  return Buffer.from([v]);
}
function turnBuf(n: number): Buffer {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}
const pawnId = (color: number, idx: number) => (color << 4) | (idx & 0x0f);

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const gameEngine = anchor.workspace.GameEngine as any;
  const predMarket = anchor.workspace.PredictionMarket as any;
  const authority = (provider.wallet as anchor.Wallet).payer;

  console.log("🚀 Starting Defnet Gameplay & Betting Blueprint...");

  // Hardcoded for the game we just created
  const GAME_ID = 1;
  const MARKET_IDX = 0; // 0 = Red
  const COLOR_RED = 0;

  // --- PDAs ---
  const [gamePda] = PublicKey.findProgramAddressSync([Buffer.from("game"), gameIdBuf(GAME_ID)], gameEngine.programId);
  const [marketPda] = PublicKey.findProgramAddressSync([Buffer.from("market"), gameIdBuf(GAME_ID), u8Buf(MARKET_IDX)], predMarket.programId);
  const [vaultPda] = PublicKey.findProgramAddressSync([Buffer.from("vault"), gameIdBuf(GAME_ID), u8Buf(MARKET_IDX)], predMarket.programId);
  const [positionPda] = PublicKey.findProgramAddressSync([Buffer.from("position"), marketPda.toBuffer(), authority.publicKey.toBuffer()], predMarket.programId);

  // Fetch the active GameState to get the turn number and active agent
  const gameState = await gameEngine.account.gameState.fetch(gamePda);
  const currentTurn = gameState.turnNumber;
  
  // Note: For this script to sign the move, your local wallet needs to temporarily be the "Red Agent" 
  // (In a real Deforge environment, the AI agent's specific keypair would sign this)
  const [vrfRequestPda] = PublicKey.findProgramAddressSync([Buffer.from("vrf_request"), gameIdBuf(GAME_ID), turnBuf(currentTurn)], gameEngine.programId);

  // Fetch the Vault Token Account directly to see exactly what mint it holds!
  const vaultTokenAccount = await getAccount(provider.connection, vaultPda);
  const dummyMint = vaultTokenAccount.mint;
  console.log(`🪙 Found Dummy Mint from Vault: ${dummyMint.toBase58()}`);

  // ==========================================
  // PHASE 1: BETTING (Buying Shares)
  // ==========================================
  console.log("\n📈 PHASE 1: Placing a Bet on Red...");
  
  // Get or create ATA for our dummy token
  const userTokenAccount = await getOrCreateAssociatedTokenAccount(
    provider.connection,
    authority,
    dummyMint,
    authority.publicKey
  );

  // Mint ourselves 100 Dummy Tokens so we have money to bet!
  console.log("   -> Minting 100 dummy tokens to wallet for betting...");
  await mintTo(
    provider.connection,
    authority,
    dummyMint,
    userTokenAccount.address,
    authority,
    100 * 1_000_000 // 100 Tokens (6 decimals)
  );

  try {
    const betTx = await predMarket.methods
      .buyShares({ yes: {} }, new BN(10 * 1_000_000), new BN(1)) // Betting 10 $AUTO
      .accounts({
        market: marketPda,
        userPosition: positionPda,
        vault: vaultPda,
        userTokenAccount: userTokenAccount.address,
        user: authority.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .rpc();
    console.log(`✅ Bet Placed! TX: https://explorer.solana.com/tx/${betTx}?cluster=devnet`);
  } catch (e) {
    console.log("⚠️ Bet failed. Check the logs below:");
    console.log(e);
  }
 // ==========================================
  // PHASE 2: THE SWITCHBOARD MOVE (Commit & Reveal)
  // ==========================================
  console.log("\n🎲 PHASE 2: Executing Turn via Switchboard VRF...");

  // 1. Setup Switchboard
  console.log("   -> Fetching Switchboard IDL from Devnet...");
  const sbProgramId = new PublicKey("SBondMDrcV3K4kxZR1HNVT7osZxAHVHgYXL5Ze1oMUv");
  const sbIdl = await anchor.Program.fetchIdl(sbProgramId, provider);
  const sbProgram = new anchor.Program(sbIdl!, provider) as any;

  // The ACTUAL live Devnet Queue for Switchboard On-Demand
  const queuePubkey = new PublicKey("FfD96yeXs4cxZshoPPSKhSPgVQxLAJUT3gefgh84m1Di");
  const rngKp = Keypair.generate();
  
  // We define our compute limit explicitly once
  const cuLimitIx = ComputeBudgetProgram.setComputeUnitLimit({ units: 500_000 });

  console.log("   -> Creating Switchboard Randomness Account...");
  const [randomness, createIx] = await sb.Randomness.create(sbProgram, rngKp, queuePubkey);
  
  // Using standard web3.Transaction bypasses all the Switchboard SDK bugs
  const createTx = new web3.Transaction().add(cuLimitIx, createIx);
  // provider.sendAndConfirm automatically waits for the block to finalize!
  await provider.sendAndConfirm(createTx, [rngKp]);
  console.log("   ✅ Randomness Account Created!");

  // 2. Commit Phase (Request Roll)
  console.log("   -> Committing to Roll...");
  const commitIx = await randomness.commitIx(queuePubkey);
  const requestRollIx = await gameEngine.methods
    .requestRoll(pawnId(COLOR_RED, 0)) // Moving Red's first pawn
    .accounts({
      gameState: gamePda,
      vrfRequest: vrfRequestPda,
      randomnessAccount: rngKp.publicKey,
      agent: authority.publicKey, // Fails if authority isn't the registered Red agent
      systemProgram: SystemProgram.programId,
    })
    .instruction();

  const commitTx = new web3.Transaction().add(cuLimitIx, commitIx, requestRollIx);
  
  try {
    const commitSig = await provider.sendAndConfirm(commitTx);
    console.log(`   ✅ Commit Successful: https://explorer.solana.com/tx/${commitSig}?cluster=devnet`);

    // 3. Wait for Oracle & Reveal Phase
    console.log("   -> Waiting 3 seconds for Switchboard Oracle...");
    await new Promise((resolve) => setTimeout(resolve, 3000));

    console.log("   -> Revealing Roll...");
    const revealIx = await randomness.revealIx();
    const fulfillRollIx = await gameEngine.methods
      .fulfillRoll()
      .accounts({
        gameState: gamePda,
        vrfRequest: vrfRequestPda,
        randomnessAccount: rngKp.publicKey,
        crank: authority.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    const revealTx = new web3.Transaction().add(cuLimitIx, revealIx, fulfillRollIx);
    const revealSig = await provider.sendAndConfirm(revealTx);
    console.log(`   ✅ Reveal Successful! Pawn Moved! TX: https://explorer.solana.com/tx/${revealSig}?cluster=devnet`);

  } catch (e) {
    console.log("❌ Move failed. Check the logs below:");
    console.log(e);
  }

  console.log("\n🎉 Gameplay Blueprint Complete.");
}

main().catch(console.error);

// ==========================================
// BLUEPRINTS FOR BACKEND / FRONTEND
// ==========================================

/* BLUEPRINT A: Ending the Game 
  (Deforge Agent fires this when it sees the GameEnded event)
  
  async function triggerEndGame(gameId: number) {
    const remainingAccounts = [0, 1, 2, 3].map(i => ({
      pubkey: PublicKey.findProgramAddressSync([Buffer.from("market"), gameIdBuf(gameId), u8Buf(i)], predMarket.programId)[0],
      isWritable: true,
      isSigner: false,
    }));

    await gameEngine.methods.endGame()
      .accounts({
        registry: registryPda,
        gameState: gamePda,
        predictionMarketProgram: predMarket.programId,
        crank: agent.publicKey,
      })
      .remainingAccounts(remainingAccounts)
      .rpc();
  }
*/

/* BLUEPRINT B: User Claiming Payout 
  (Frontend fires this when user clicks "Claim Winnings")
  
  async function claimWinnings(gameId: number, marketIdx: number) {
    await predMarket.methods.claimPayout()
      .accounts({
        market: marketPda,
        userPosition: positionPda,
        vault: vaultPda,
        userTokenAccount: userAta,
        user: user.publicKey,
        gameState: gamePda,
        gameEngineProgram: gameEngine.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .rpc();
  }
*/