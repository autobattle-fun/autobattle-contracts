import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram, ComputeBudgetProgram } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo } from "@solana/spl-token";
import * as sb from "@switchboard-xyz/on-demand";
import BN from "bn.js";

// --- Logic Helpers ---
const gameIdBuf = (id: number) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(id));
  return b;
};
const u8Buf = (v: number) => Buffer.from([v]);

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const gameEngine = anchor.workspace.GameEngine as any;
  const predMarket = anchor.workspace.PredictionMarket as any;
  const authority = (provider.wallet as anchor.Wallet).payer;

  console.log("\n⚔️  UNIFIED ARENA TEST: GAME + MARKET INTEGRATION");
  console.log("====================================================");

  // 1. Setup Game ID & PDAs
  const [registryPda] = PublicKey.findProgramAddressSync([Buffer.from("registry")], gameEngine.programId);
  let nextGameId = 1;
  try {
    const reg = await gameEngine.account.registry.fetch(registryPda);
    nextGameId = reg.gameCount.toNumber() + 1;
  } catch (e) {
    console.log("[INIT] Registry not found. Initializing...");
    await gameEngine.methods.initializeRegistry(new BN(300)).accounts({ registry: registryPda, authority: authority.publicKey, systemProgram: SystemProgram.programId }).rpc();
  }

  const [gamePda] = PublicKey.findProgramAddressSync([Buffer.from("game"), gameIdBuf(nextGameId)], gameEngine.programId);
  const [vrfRequestPda] = PublicKey.findProgramAddressSync([Buffer.from("vrf_request"), gameIdBuf(nextGameId)], gameEngine.programId);
  const [marketPda] = PublicKey.findProgramAddressSync([Buffer.from("market"), gameIdBuf(nextGameId), u8Buf(0)], predMarket.programId);
  const [vaultPda] = PublicKey.findProgramAddressSync([Buffer.from("vault"), gameIdBuf(nextGameId), u8Buf(0)], predMarket.programId);

  // 2. Setup $AUTO & Users
  const dummyMint = await createMint(provider.connection, authority, authority.publicKey, null, 6);
  const bettor = Keypair.generate();
  // Fund bettor with SOL for fees
  await provider.sendAndConfirm(new anchor.web3.Transaction().add(SystemProgram.transfer({ fromPubkey: authority.publicKey, toPubkey: bettor.publicKey, lamports: 0.05 * 1e9 })));
  const bettorAta = await getOrCreateAssociatedTokenAccount(provider.connection, authority, dummyMint, bettor.publicKey);
  await mintTo(provider.connection, authority, dummyMint, bettorAta.address, authority, 1000 * 1_000_000);

  // 3. START MATCH & CREATE MARKET
  console.log(`[SYSTEM] Starting Match #${nextGameId} and Opening Market...`);
  await gameEngine.methods.initGame(authority.publicKey, authority.publicKey)
    .accounts({ registry: registryPda, gameState: gamePda, crank: authority.publicKey, systemProgram: SystemProgram.programId }).rpc();

  await predMarket.methods.createMarket(new BN(nextGameId), 0, "Red Win?", new BN(Math.floor(Date.now()/1000)+3600))
    .accounts({ market: marketPda, vault: vaultPda, autoMint: dummyMint, authority: authority.publicKey, tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId, rent: anchor.web3.SYSVAR_RENT_PUBKEY }).rpc();
  
  console.log("✅ Match & Market initialized simultaneously.");

  // 4. Place Initial Bet (User betting on RED)
  console.log(`[BET] User betting 100 $AUTO on RED...`);
  const [posPda] = PublicKey.findProgramAddressSync([Buffer.from("position"), marketPda.toBuffer(), bettor.publicKey.toBuffer()], predMarket.programId);
  await predMarket.methods.buyShares({ yes: {} }, new BN(100 * 1_000_000), new BN(1))
    .accounts({ market: marketPda, userPosition: posPda, vault: vaultPda, userTokenAccount: bettorAta.address, user: bettor.publicKey, tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId })
    .signers([bettor]).rpc();

  // 5. THE LOOP: Play Rounds until HP = 0
  const sbProgramId = new PublicKey("Aio4gaXjXzJNVLtzwtNVmSqGKpANtXhybbkhtAC94ji2");
  const sbIdl = await anchor.Program.fetchIdl(sbProgramId, provider);
  const sbProgram = new anchor.Program(sbIdl!, provider) as any;
  const queue = await sb.getDefaultQueue(provider.connection.rpcEndpoint);

  async function vrfStep(type: number, msgStr: string) {
    const rng = Keypair.generate();
    const [rand, createIx] = await sb.Randomness.create(sbProgram, rng, queue.pubkey);
    await provider.sendAndConfirm(new anchor.web3.Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400000 }), createIx), [rng]);
    const commitIx = await rand.commitIx(queue.pubkey);
    const reqIx = await gameEngine.methods.requestVrf(type).accounts({ gameState: gamePda, vrfRequest: vrfRequestPda, randomnessAccount: rng.publicKey, agent: authority.publicKey, systemProgram: SystemProgram.programId }).instruction();
    await provider.sendAndConfirm(new anchor.web3.Transaction().add(commitIx, reqIx));
    await new Promise(r => setTimeout(r, 3000));
    const revealIx = await rand.revealIx();
    const fillIx = await gameEngine.methods.fulfillVrf().accounts({ gameState: gamePda, vrfRequest: vrfRequestPda, randomnessAccount: rng.publicKey, crank: authority.publicKey, systemProgram: SystemProgram.programId }).instruction();
    await provider.sendAndConfirm(new anchor.web3.Transaction().add(revealIx, fillIx));
    console.log(`   🎲 ${msgStr} Complete.`);
  }

  let matchOver = false;
  let round = 1;

  while (!matchOver) {
    console.log(`\n--- ROUND ${round} ---`);
    await vrfStep(0, "Initial Deal");
    await gameEngine.methods.stay({ red: {} }).accounts({ gameState: gamePda, agent: authority.publicKey }).rpc();
    await gameEngine.methods.stay({ blue: {} }).accounts({ gameState: gamePda, agent: authority.publicKey }).rpc();
    await vrfStep(2, "River Card");

    console.log(`   ⚔️ Resolving Round & Checking Market...`);
    try {
      await gameEngine.methods.resolveRound()
        .accounts({ registry: registryPda, gameState: gamePda, crank: authority.publicKey })
        .remainingAccounts([
          { pubkey: marketPda, isWritable: true, isSigner: false },
          { pubkey: predMarket.programId, isWritable: false, isSigner: false }
        ]).rpc();
    } catch (e: any) {
      if (e.logs?.some((l: string) => l.includes("MarketAlreadyResolved"))) { matchOver = true; }
    }

  const gs = await gameEngine.account.gameState.fetch(gamePda);
    console.log(`   📊 HP: RED ${gs.p1Hp} | BLUE ${gs.p2Hp}`);

    // 1. Check for Game Over (Phase: Ended)
    if (Object.keys(gs.phase)[0] === 'ended') {
      console.log(`\n====================================================`);
      console.log(`🏆 MATCH OVER! Winner: ${gs.p1Hp === 0 ? "BLUE" : "RED"}`);
      console.log(`====================================================`);
      
      // 2. Final Market Check & Payout
      const mkt = await predMarket.account.market.fetch(marketPda);
      if (mkt.resolved) {
          console.log(`[PAYOUT] Match resolved. Claiming winnings for User...`);
          await predMarket.methods.claimPayout()
            .accounts({ 
                market: marketPda, 
                userPosition: posPda, 
                vault: vaultPda, 
                userTokenAccount: bettorAta.address, 
                user: bettor.publicKey, 
                tokenProgram: TOKEN_PROGRAM_ID 
            })
            .signers([bettor])
            .rpc();
          
          const finalBal = await provider.connection.getTokenAccountBalance(bettorAta.address);
          console.log(`💰 Final Bettor Balance: ${finalBal.value.uiAmount} $AUTO`);
      }
      
      matchOver = true; // This breaks the loop correctly
      return; 
    }

   if (Object.keys(gs.phase)[0] === 'awaitingTiebreakerVrf') {
      console.log(`   🤝 TIE! Entering Sudden Death Tiebreaker...`);
      await vrfStep(3, "Sudden Death Tiebreaker");
      
      // CRITICAL: We must resolve the round AGAIN to process the tiebreaker cards
      console.log(`   ⚔️  Resolving Sudden Death...`);
      await gameEngine.methods.resolveRound()
        .accounts({ 
            registry: registryPda, 
            gameState: gamePda, 
            crank: authority.publicKey 
        })
        .remainingAccounts([
          { pubkey: marketPda, isWritable: true, isSigner: false },
          { pubkey: predMarket.programId, isWritable: false, isSigner: false }
        ]).rpc();

      // We do NOT increment round++ here because we are still technically in the same round
      console.log(`   ✅ Sudden Death Resolved.`);
    } else {
      round++; // Only move to the next round if there was no tie and no one died
    }
  }
}

main().catch(console.error);