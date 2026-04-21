import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { 
  TOKEN_PROGRAM_ID, 
  createMint, 
  getOrCreateAssociatedTokenAccount, 
  mintTo 
} from "@solana/spl-token";
import BN from "bn.js";

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const predMarket = anchor.workspace.PredictionMarket as any;
  const authority = (provider.wallet as anchor.Wallet).payer;

  console.log("\n====================================================");
  console.log("📈 PREDICTION MARKET SIMULATION: FULL LIFECYCLE");
  console.log("====================================================\n");

  // 1. Setup Dummy Currency ($AUTO)
  console.log("[SETUP] Minting $AUTO tokens...");
  const dummyMint = await createMint(provider.connection, authority, authority.publicKey, null, 6);
  const userA = Keypair.generate(); // The Bull
  const userB = Keypair.generate(); // The Bear

  // Fund users with SOL and $AUTO
  for (const user of [userA, userB]) {
    const transferIx = anchor.web3.SystemProgram.transfer({
      fromPubkey: authority.publicKey,
      toPubkey: user.publicKey,
      lamports: 0.1 * anchor.web3.LAMPORTS_PER_SOL, // 0.1 SOL is plenty for fees
    });
    
    const tx = new anchor.web3.Transaction().add(transferIx);
    await provider.sendAndConfirm(tx);
    
    const ata = await getOrCreateAssociatedTokenAccount(provider.connection, authority, dummyMint, user.publicKey);
    await mintTo(provider.connection, authority, dummyMint, ata.address, authority, 1000 * 1_000_000);
  }
  console.log("✅ Users funded with 1,000 $AUTO each.\n");

  // 2. Create Market
  const gameId = new BN(Date.now()); // Unique ID for this test
  const marketIdx = 0;
  const [marketPda] = PublicKey.findProgramAddressSync([Buffer.from("market"), gameId.toArrayLike(Buffer, "le", 8), Buffer.from([marketIdx])], predMarket.programId);
  const [vaultPda] = PublicKey.findProgramAddressSync([Buffer.from("vault"), gameId.toArrayLike(Buffer, "le", 8), Buffer.from([marketIdx])], predMarket.programId);

  console.log(`[ACTION] Creating Market for Game #${gameId.toNumber()}...`);
  await predMarket.methods
    .createMarket(gameId, marketIdx, "Will Red Agent win the match?", new BN(Math.floor(Date.now() / 1000) + 3600))
    .accounts({
      market: marketPda,
      vault: vaultPda,
      autoMint: dummyMint,
      authority: authority.publicKey,
      tokenProgram: TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .rpc();
  console.log("✅ Market Created.\n");

  // 3. User A Buys YES (Red Win)
  console.log("[TRADE] User A betting 500 $AUTO on YES...");
  const ataA = (await getOrCreateAssociatedTokenAccount(provider.connection, authority, dummyMint, userA.publicKey)).address;
  const [posAPda] = PublicKey.findProgramAddressSync([Buffer.from("position"), marketPda.toBuffer(), userA.publicKey.toBuffer()], predMarket.programId);

  await predMarket.methods
    .buyShares({ yes: {} }, new BN(500 * 1_000_000), new BN(1)) // 1 = Min shares (slippage)
    .accounts({
      market: marketPda,
      userPosition: posAPda,
      vault: vaultPda,
      userTokenAccount: ataA,
      user: userA.publicKey,
      tokenProgram: TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .signers([userA])
    .rpc();

  // 4. User B Buys NO (Blue Win)
  console.log("[TRADE] User B betting 200 $AUTO on NO...");
  const ataB = (await getOrCreateAssociatedTokenAccount(provider.connection, authority, dummyMint, userB.publicKey)).address;
  const [posBPda] = PublicKey.findProgramAddressSync([Buffer.from("position"), marketPda.toBuffer(), userB.publicKey.toBuffer()], predMarket.programId);

  await predMarket.methods
    .buyShares({ no: {} }, new BN(200 * 1_000_000), new BN(1))
    .accounts({
      market: marketPda,
      userPosition: posBPda,
      vault: vaultPda,
      userTokenAccount: ataB,
      user: userB.publicKey,
      tokenProgram: TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .signers([userB])
    .rpc();

  // 5. Resolve Market
  console.log("\n[RESULT] Game Finished! Red wins. Resolving Market...");
  // In this test, we call it directly as authority. In prod, GameEngine calls this via CPI.
  await predMarket.methods
    .resolveMarket({ yes: {} }) // Tell Anchor explicitly to use the 'Yes' (or 'Red') variant
    .accounts({
      market: marketPda,
      authority: authority.publicKey,
    })
    .rpc();
  console.log("✅ Market Resolved: RED WINS.\n");

  // 6. User A Claims Payout
  console.log("[PAYOUT] User A claiming winnings...");
  const beforeBalance = (await provider.connection.getTokenAccountBalance(ataA)).value.uiAmount;
  
  await predMarket.methods
    .claimPayout()
    .accounts({
      market: marketPda,
      userPosition: posAPda,
      vault: vaultPda,
      userTokenAccount: ataA,
      user: userA.publicKey,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .signers([userA])
    .rpc();

  const afterBalance = (await provider.connection.getTokenAccountBalance(ataA)).value.uiAmount;
  console.log(`✅ Claim Successful! User A Balance: ${beforeBalance} -> ${afterBalance} $AUTO`);
  console.log(`🔥 Total Profit: ${(afterBalance! - beforeBalance!).toFixed(2)} $AUTO\n`);
  
  console.log("====================================================");
  console.log("🏆 SIMULATION COMPLETE");
  console.log("====================================================\n");
}

main().catch(console.error);