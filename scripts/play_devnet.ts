import * as anchor from "@coral-xyz/anchor";
import BN from "bn.js";
import { Keypair, PublicKey, SystemProgram, ComputeBudgetProgram } from "@solana/web3.js";
import { getOrCreateAssociatedTokenAccount, TOKEN_PROGRAM_ID, mintTo, createMint } from "@solana/spl-token";
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

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const gameEngine = anchor.workspace.GameEngine as any;
  const predMarket = anchor.workspace.PredictionMarket as any;
  const authority = (provider.wallet as anchor.Wallet).payer;

  console.log("\n====================================================");
  console.log("🚀 AUTO-BATTLE ARENA: SCALING DEATHMATCH");
  console.log("====================================================");
  console.log(`🔑 Wallet: ${authority.publicKey.toBase58()}`);

  // PHASE 1: INIT GAME
  const [registryPda] = PublicKey.findProgramAddressSync([Buffer.from("registry")], gameEngine.programId);
  let nextGameId = 1;
  try {
    const reg = await gameEngine.account.registry.fetch(registryPda);
    nextGameId = reg.gameCount.toNumber() + 1;
  } catch (e) {}

  const [gamePda] = PublicKey.findProgramAddressSync([Buffer.from("game"), gameIdBuf(nextGameId)], gameEngine.programId);
  const [vrfRequestPda] = PublicKey.findProgramAddressSync([Buffer.from("vrf_request"), gameIdBuf(nextGameId)], gameEngine.programId);

  console.log(`\n[SYSTEM] Initializing Game #${nextGameId}...`);
  await gameEngine.methods.initGame(authority.publicKey, authority.publicKey)
    .accounts({ registry: registryPda, gameState: gamePda, crank: authority.publicKey, systemProgram: SystemProgram.programId })
    .rpc();
  console.log(`✅ Game State Created at ${gamePda.toBase58().slice(0, 8)}...`);

  // PHASE 2: INIT MARKET
  console.log(`[MARKET] Setting up Prediction Market & $AUTO Mint...`);
  const [marketPda] = PublicKey.findProgramAddressSync([Buffer.from("market"), gameIdBuf(nextGameId), u8Buf(0)], predMarket.programId);
  const [vaultPda] = PublicKey.findProgramAddressSync([Buffer.from("vault"), gameIdBuf(nextGameId), u8Buf(0)], predMarket.programId);
  const dummyMint = await createMint(provider.connection, authority, authority.publicKey, null, 6);
  
  await predMarket.methods.createMarket(new BN(nextGameId), 0, "Red Win?", new BN(Math.floor(Date.now()/1000)+86400))
    .accounts({ market: marketPda, vault: vaultPda, autoMint: dummyMint, authority: authority.publicKey, tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId, rent: anchor.web3.SYSVAR_RENT_PUBKEY })
    .rpc();
  console.log(`✅ Market Live: "Will Red Win?"`);

  // VRF HELPER
  const sbProgramId = new PublicKey("Aio4gaXjXzJNVLtzwtNVmSqGKpANtXhybbkhtAC94ji2");
  const sbIdl = await anchor.Program.fetchIdl(sbProgramId, provider);
  const sbProgram = new anchor.Program(sbIdl!, provider) as any;
  const queue = await sb.getDefaultQueue(provider.connection.rpcEndpoint);

  async function executeVrfPhase(rollType: number, phaseName: string) {
    process.stdout.write(`   🎲 ${phaseName} | Requesting VRF... `);
    const rngKp = Keypair.generate();
    const [randomness, createIx] = await sb.Randomness.create(sbProgram, rngKp, queue.pubkey);
    await provider.sendAndConfirm(new anchor.web3.Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 500_000 }), createIx), [rngKp]);
    
    const commitIx = await randomness.commitIx(queue.pubkey);
    const requestRollIx = await gameEngine.methods.requestVrf(rollType).accounts({ gameState: gamePda, vrfRequest: vrfRequestPda, randomnessAccount: rngKp.publicKey, agent: authority.publicKey, systemProgram: SystemProgram.programId }).instruction();
    await provider.sendAndConfirm(new anchor.web3.Transaction().add(commitIx, requestRollIx));
    
    process.stdout.write(`Oracle working... `);
    await new Promise((r) => setTimeout(r, 3000));
    
    const revealIx = await randomness.revealIx();
    const fulfillRollIx = await gameEngine.methods.fulfillVrf().accounts({ gameState: gamePda, vrfRequest: vrfRequestPda, randomnessAccount: rngKp.publicKey, crank: authority.publicKey, systemProgram: SystemProgram.programId }).instruction();
    await provider.sendAndConfirm(new anchor.web3.Transaction().add(revealIx, fulfillRollIx));
    
    const gs = await gameEngine.account.gameState.fetch(gamePda);
    console.log(`Done! -> [Score: R ${gs.p1Score} | B ${gs.p2Score}]`);
  }

  // MATCH LOOP
  let matchOver = false;
  let round = 1;

  while (!matchOver) {
    const damagePower = 1 << (round - 1);
    console.log(`\n--- ROUND ${round} | STAKES: ${damagePower} HP ---`);
    
    // 1. Initial Deal
    await executeVrfPhase(0, "DEAL");

    // 2. Turns (Simulation: Both hit once)
    console.log(`   🏃 RED Move: HIT`);
    await executeVrfPhase(1, "HIT (Red)");
    
    console.log(`   🏃 BLUE Move: HIT`);
    await executeVrfPhase(1, "HIT (Blue)");
    
    // 3. Stays
    console.log(`   ✋ Action: BOTH STAY`);
    await gameEngine.methods.stay({ red: {} }).accounts({ gameState: gamePda, agent: authority.publicKey }).rpc();
    await gameEngine.methods.stay({ blue: {} }).accounts({ gameState: gamePda, agent: authority.publicKey }).rpc();
    
    // 4. Final Reveal
    await executeVrfPhase(2, "THE RIVER (Final Reveal)");

    const gs = await gameEngine.account.gameState.fetch(gamePda);
    console.log(`\n   📊 FINAL HAND: [Red: ${gs.p1Score}] vs [Blue: ${gs.p2Score}]`);

    // 5. Resolve
    process.stdout.write(`   ⚔️  Resolving Damage... `);
    try {
      await gameEngine.methods.resolveRound()
        .accounts({ registry: registryPda, gameState: gamePda, crank: authority.publicKey })
        .remainingAccounts([{ pubkey: marketPda, isWritable: true, isSigner: false }, { pubkey: predMarket.programId, isWritable: false, isSigner: false }])
        .rpc();
      console.log(`Calculated!`);
    } catch (e: any) {
        if (e.logs?.some((l: string) => l.includes("MarketAlreadyResolved"))) {
            console.log(`Market Settled!`);
        } else {
            console.log(`Error or Tie Detected.`);
        }
    }

    const finalGs = await gameEngine.account.gameState.fetch(gamePda);
    console.log(`   ❤️  RED HP: ${finalGs.p1Hp}/10`);
    console.log(`   💙  BLUE HP: ${finalGs.p2Hp}/10`);

    if (finalGs.p1Hp === 0 || finalGs.p2Hp === 0) {
      const winner = finalGs.p1Hp === 0 ? "BLUE" : "RED";
      console.log(`\n====================================================`);
      console.log(`🏆 MATCH OVER! WINNER: ${winner}`);
      console.log(`💰 Prediction Market Payouts Unlocked.`);
      console.log(`====================================================\n`);
      matchOver = true;
    } else {
      console.log(`\n[SYSTEM] No fatal blow. Scaling stakes...`);
      round++;
    }
  }
}

main().catch(console.error);