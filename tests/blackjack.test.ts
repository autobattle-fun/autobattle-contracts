import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import BN from "bn.js";
import { expect } from "chai";

const REGISTRY_SEED = Buffer.from("registry");
const GAME_SEED = Buffer.from("game");
const VRF_SEED = Buffer.from("vrf_request");

function gameIdBuf(id: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(id));
  return b;
}

const CARDS = {
  ACE: 0,   
  TWO: 1,   
  NINE: 8,  
  TEN: 9,   
  KING: 12, 
};

describe("blackjack-physics-engine", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const gameEngine = anchor.workspace.GameEngine as Program;
  const crank = (provider.wallet as anchor.Wallet).payer;
  const agentRed = Keypair.generate();
  const agentBlue = Keypair.generate();

  let registryPda: PublicKey;
  let gamePda: PublicKey;
  let vrfPda: PublicKey;

  before(async () => {
    // 1. Airdrop SOL so the agents can pay PDA rent!
    const signers = [agentRed, agentBlue];
    await Promise.all(
      signers.map((kp) =>
        provider.connection.requestAirdrop(kp.publicKey, 10 * LAMPORTS_PER_SOL)
          .then((sig) => provider.connection.confirmTransaction(sig))
      )
    );

    [registryPda] = PublicKey.findProgramAddressSync([REGISTRY_SEED], gameEngine.programId);
    
    let nextGameId = 1;
    try {
      await gameEngine.methods.initializeRegistry(new BN(300))
        .accounts({ registry: registryPda, authority: crank.publicKey, systemProgram: SystemProgram.programId })
        .rpc();
    } catch (e) {
      const reg = await gameEngine.account.registry.fetch(registryPda);
      nextGameId = reg.gameCount.toNumber() + 1;
    }

    [gamePda] = PublicKey.findProgramAddressSync([GAME_SEED, gameIdBuf(nextGameId)], gameEngine.programId);
    [vrfPda] = PublicKey.findProgramAddressSync([VRF_SEED, gameIdBuf(nextGameId)], gameEngine.programId);

    await gameEngine.methods.initGame(agentRed.publicKey, agentBlue.publicKey)
      .accounts({ registry: registryPda, gameState: gamePda, crank: crank.publicKey, systemProgram: SystemProgram.programId })
      .rpc();
  });

  it("Initial Deal: Smart Ace & Face Card Logic", async () => {
    await gameEngine.methods.mockRequestVrf(0)
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, randomnessAccount: crank.publicKey, agent: agentRed.publicKey, systemProgram: SystemProgram.programId })
      .signers([agentRed]).rpc();

    await gameEngine.methods.mockFulfillVrf(Buffer.from([CARDS.ACE, CARDS.KING]))
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, crank: crank.publicKey }).rpc();

    const gs = await gameEngine.account.gameState.fetch(gamePda);
    expect(gs.p1Score).to.eq(11); 
    expect(gs.p2Score).to.eq(10); 
  });

  it("Red Hits, Turn Passes to Blue, Blue Stays, Red Hits & Downgrades Ace", async () => {
    // 1. Red requests hit
    await gameEngine.methods.mockRequestVrf(1)
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, randomnessAccount: crank.publicKey, agent: agentRed.publicKey, systemProgram: SystemProgram.programId })
      .signers([agentRed]).rpc();

    // Red draws a 9. Score -> 20. 
    await gameEngine.methods.mockFulfillVrf(Buffer.from([CARDS.NINE, 0]))
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, crank: crank.publicKey }).rpc();

    let gs = await gameEngine.account.gameState.fetch(gamePda);
    expect(gs.p1Score).to.eq(20);
    
    // TURN PASSED TO BLUE!
    expect(gs.activePlayer).to.deep.equal({ blue: {} }); 

    // 2. Blue is intimidated by Red's 20 and chooses to Stay
    await gameEngine.methods.stay({ blue: {} })
      .accounts({ gameState: gamePda, agent: agentBlue.publicKey })
      .signers([agentBlue]).rpc();

    gs = await gameEngine.account.gameState.fetch(gamePda);
    expect(gs.p2Stayed).to.be.true;
    
    // TURN PASSES BACK TO RED
    expect(gs.activePlayer).to.deep.equal({ red: {} }); 

    // 3. Red gets greedy and hits AGAIN
    await gameEngine.methods.mockRequestVrf(1)
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, randomnessAccount: crank.publicKey, agent: agentRed.publicKey, systemProgram: SystemProgram.programId })
      .signers([agentRed]).rpc();

    // Red draws a TWO. 20 + 2 = 22. 
    // The Smart Ace should immediately downgrade from 11 to 1, saving Red!
    // New score: 1 + 9 + 2 = 12.
    await gameEngine.methods.mockFulfillVrf(Buffer.from([CARDS.TWO, 0]))
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, crank: crank.publicKey }).rpc();

    gs = await gameEngine.account.gameState.fetch(gamePda);
    expect(gs.p1Score).to.eq(12); 
    expect(gs.p1Aces).to.eq(0);   
  });

  it("Red Stays, triggering Final Reveal phase", async () => {
    // Blue already stayed. Red stays now.
    await gameEngine.methods.stay({ red: {} })
      .accounts({ gameState: gamePda, agent: agentRed.publicKey })
      .signers([agentRed]).rpc();

    const gs = await gameEngine.account.gameState.fetch(gamePda);
    expect(gs.phase).to.deep.equal({ awaitingFinalRevealVrf: {} });
  });

  it("Final Reveal: Forces a Tiebreaker scenario", async () => {
    await gameEngine.methods.mockRequestVrf(2)
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, randomnessAccount: crank.publicKey, agent: agentRed.publicKey, systemProgram: SystemProgram.programId })
      .signers([agentRed]).rpc(); 

    // Red currently has 12. Blue has 10.
    // Red draws a 7 (Byte 6 -> 7). New Score: 19
    // Blue draws a 9 (Byte 8 -> 9). New Score: 19
    await gameEngine.methods.mockFulfillVrf(Buffer.from([6, 8]))
      .accounts({ gameState: gamePda, vrfRequest: vrfPda, crank: crank.publicKey }).rpc();

    let gs = await gameEngine.account.gameState.fetch(gamePda);
    expect(gs.p1Score).to.eq(19);
    expect(gs.p2Score).to.eq(19);

    await gameEngine.methods.resolveRound()
      .accounts({ registry: registryPda, gameState: gamePda, crank: crank.publicKey })
      .rpc();

    gs = await gameEngine.account.gameState.fetch(gamePda);
    
    // Tiebreaker successful! No damage dealt, Sudden Death triggered.
    expect(gs.p1Hp).to.eq(10); 
    expect(gs.phase).to.deep.equal({ awaitingTiebreakerVrf: {} });
  });
});