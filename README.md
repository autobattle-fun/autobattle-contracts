# Ludo On-Chain — Solana Smart Contracts

## Programs

| Program | Description |
|---|---|
| `game-engine` | Core Ludo game logic, turn management, VRF integration |
| `prediction-market` | LMSR AMM, $AUTO share trading, payout settlement |

---

## Account map

```
Registry          [b"registry"]
└── GameState     [b"game", game_id: u64]
    ├── VRFRequest  [b"vrf_request", game_id, agent_pubkey]  ← short-lived
    └── (referenced by)
        Market    [b"market", game_id, market_index: u8]
        ├── Vault (SPL TokenAccount) [b"vault", game_id, market_index]
        └── UserPosition  [b"position", market_key, user_key]
```

---

## Instruction flow

### Game engine

```
initialize_registry()         ← deploy once
    ↓
init_game(agents[4])          ← crank after cooldown
    ↓
request_roll(pawn_id)         ← active agent commits pawn
    ↓  [VRF oracle callback]
fulfill_roll(request_id, randomness)  ← moves pawn, emits events
    ↓  [repeat until winner]
end_game()                    ← finalises, sets next_game_starts_at
```

### Prediction market

```
create_market(game_id, market_index, question, expires_at)
    ↓
buy_shares(outcome, amount_in, min_shares_out)   ← users trade
sell_shares(outcome, shares_in, min_amount_out)  ← users exit
    ↓  [game ends]
resolve_market(outcome)       ← authority or game engine CPI
    ↓
claim_payout()                ← winners collect $AUTO
    OR
refund_expired()              ← anyone cranks if unresolved after 2hr grace
```

---

## Key design decisions

### Randomness (VRF)
- Agent calls `request_roll(pawn_id)` — commits to pawn BEFORE seeing roll
- `request_id` derived from `hash(game_id || turn_number || agent_pubkey)`
- VRF oracle fulfills → `fulfill_roll` verifies `request_id` matches stored value
- **Recommended**: Switchboard On-Demand VRF (most battle-tested on Solana)
- Replace the `request_id` derivation in `request_roll` with the actual Switchboard VRF account pubkey

### LMSR pricing
- Starting price: 0.5 $AUTO per share (50/50 at market open)
- `b` parameter (liquidity depth) = 100 $AUTO — tune this before mainnet
- Current implementation uses linear price approximation (safe for small trades)
- For production: replace `lmsr.rs` with full Taylor series exp/ln or use `fixed` crate

### Fund safety
- `UserPosition` never auto-closed — only user can close after `claimed = true`
- `claims_remaining` counter on `Market` — vault closeable only at 0
- `refund_expired` — permissionless, callable 2hr after expiry if unresolved
- `upgrade_locked` on `GameState` — social signal to multisig to not upgrade mid-game

### Cooldown
- `next_game_starts_at` stored in `GameState` (set on `end_game`)
- `init_game` checks `Clock::get()?.unix_timestamp >= next_game_starts_at`
- Fully trustless — no backend can skip the cooldown

---

## TODO before mainnet

- [ ] Replace `request_id` derivation with Switchboard VRF account pubkey
- [ ] Add CPI from `game_engine::end_game` → `prediction_market::resolve_market` for win markets
- [ ] Add CPI back from prediction market → game engine to clear `upgrade_locked` when `claims_remaining == 0`
- [ ] Cache `next_game_starts_at` in `Registry` (avoids extra account fetch in `init_game`)
- [ ] Replace linear LMSR approximation with proper fixed-point exp/ln
- [ ] Add protocol fee (1-2%) on `buy_shares` / `sell_shares` → treasury account
- [ ] Upgrade authority → Squads multisig
- [ ] Write Anchor tests for: capture logic, home stretch overshoot, refund_expired, double-claim attempt
- [ ] Fuzz `compute_move` with all 52 board positions × 6 rolls × 4 colors

---

## Local development

```bash
# Install
anchor build

# Test
anchor test

# Deploy devnet
anchor deploy --provider.cluster devnet
```
