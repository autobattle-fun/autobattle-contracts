import * as anchor from "@coral-xyz/anchor";
const { BN, web3 } = anchor;
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { createMint, TOKEN_PROGRAM_ID } from "@solana/spl-token";

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
  // Set up the provider (uses your Anchor.toml settings: devnet + local wallet)
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const gameEngine = anchor.workspace.GameEngine as any;
  const predMarket = anchor.workspace.PredictionMarket as any;
  const authority = (provider.wallet as anchor.Wallet).payer;

  console.log("🚀 Starting Devnet Prototype Agent...");
  console.log(`🔑 Wallet: ${authority.publicKey.toBase58()}`);

  // 1. Check or Initialize Registry
  const [registryPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("registry")],
    gameEngine.programId
  );

  let registry;
  try {
    registry = await gameEngine.account.registry.fetch(registryPda);
    console.log(`✅ Registry found. Current Game Count: ${registry.gameCount.toNumber()}`);
  } catch (e) {
    console.log("⚙️ Registry not found. Initializing now...");
    await gameEngine.methods
      .initializeRegistry(new BN(300)) // 5 minute cooldown
      .accounts({
        registry: registryPda,
        authority: authority.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();
    registry = await gameEngine.account.registry.fetch(registryPda);
    console.log("✅ Registry Initialized!");
  }

  // 2. Start a New Game
  const nextGameId = registry.gameCount.toNumber() + 1;
  const [gamePda] = PublicKey.findProgramAddressSync(
    [Buffer.from("game"), gameIdBuf(nextGameId)],
    gameEngine.programId
  );

  // Generate random keys for the 4 AI agents for testing
  const agents = [Keypair.generate(), Keypair.generate(), Keypair.generate(), Keypair.generate()];

  console.log(`🎲 Starting Game ID: ${nextGameId}...`);
  
  // Note: If gameCount > 0, we need to pass the PREVIOUS game PDA as a remaining account
  // to satisfy the cooldown check.
  const remainingAccounts = [];
  if (registry.gameCount.toNumber() > 0) {
    const [prevGamePda] = PublicKey.findProgramAddressSync(
      [Buffer.from("game"), gameIdBuf(registry.gameCount.toNumber())],
      gameEngine.programId
    );
    remainingAccounts.push({ pubkey: prevGamePda, isWritable: false, isSigner: false });
  }

  await gameEngine.methods
    .initGame(agents[0].publicKey, agents[1].publicKey, agents[2].publicKey, agents[3].publicKey)
    .accounts({
      registry: registryPda,
      gameState: gamePda,
      crank: authority.publicKey,
      systemProgram: SystemProgram.programId,
    })
    .remainingAccounts(remainingAccounts)
    .rpc();

  console.log(`✅ Game ${nextGameId} Initialized! PDA: ${gamePda.toBase58()}`);

  // 3. Create a Dummy Token Mint (For testing market liquidity on Devnet)
  console.log("🪙 Minting Dummy $AUTO Token for markets...");
  const dummyMint = await createMint(
    provider.connection,
    authority,
    authority.publicKey,
    null,
    6 // 6 decimals like pump.fun
  );
  console.log(`✅ Dummy Mint Created: ${dummyMint.toBase58()}`);

  // 4. Bundle and Create the 4 Default Markets
  console.log("📊 Bundling 4 Prediction Markets into one transaction...");
  const tx = new web3.Transaction();
  const colors = ["Red", "Blue", "Yellow", "Green"];
  const expiresAt = Math.floor(Date.now() / 1000) + 86400; // 24 hours from now

  for (let i = 0; i < 4; i++) {
    const [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), gameIdBuf(nextGameId), u8Buf(i)],
      predMarket.programId
    );
    const [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), gameIdBuf(nextGameId), u8Buf(i)],
      predMarket.programId
    );

    const ix = await predMarket.methods
      .createMarket(new BN(nextGameId), i, `Will ${colors[i]} win?`, new BN(expiresAt))
      .accounts({
        market: marketPda,
        vault: vaultPda,
        autoMint: dummyMint,
        authority: authority.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .instruction(); // <-- Notice we use .instruction() instead of .rpc() to bundle them!

    tx.add(ix);
  }

  // Send the bundled transaction
  const txSig = await provider.sendAndConfirm(tx);
  console.log(`✅ All 4 Markets successfully created!`);
  console.log(`🔗 Transaction Signature: https://explorer.solana.com/tx/${txSig}?cluster=devnet`);
  console.log("🎉 Run complete. Devnet is fully populated and ready for UI testing.");
}

main().catch((err) => {
  console.error("❌ Script Error:", err);
});