---
title: "[AGENT] Phase 1d T2: Define NATS wire schemas for Action/OrderUpdate"
labels: agent-task,phase-1d,difficulty-medium,area-core
---

## Task
Formalize the over-the-wire schemas for `Action`, `OrderUpdate`, `FairValueMessage`, and `MdFrame` with a `schema_version: u16` field and bincode serialization. Document in `docs/NATS_SUBJECTS.md`.

## Context
Maps to `PROJECT_PLAN.md` §1.2. Enables dual-version support across engine/strategy upgrades.

## Files to Touch
- `crates/nats-transport/src/schemas.rs` (new)
- `docs/NATS_SUBJECTS.md` (new)
- `crates/core/src/action.rs` — wrap `Action` in a versioned envelope (optional; can stay in nats-transport)

## Cursor prompt

```
Define versioned wire schemas for NATS messages.

1. In crates/nats-transport/src/schemas.rs:

    use serde::{Deserialize, Serialize};
    use trading_core::{Action, InstrumentId};
    // also import Event, FairValueMessage from their crates

    pub const SCHEMA_VERSION: u16 = 1;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Envelope<T> {
        pub schema_version: u16,
        pub ts_ns: u64,
        pub payload: T,
    }

    impl<T> Envelope<T> {
        pub fn new(payload: T) -> Self {
            Self {
                schema_version: SCHEMA_VERSION,
                ts_ns: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
                payload,
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ActionMsg {
        pub action_id: uuid::Uuid,        // idempotency key
        pub strategy_id: String,
        pub action: Action,
    }

    pub fn encode<T: Serialize>(msg: &Envelope<T>) -> Vec<u8> {
        bincode::serialize(msg).expect("bincode encode")
    }

    pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<Envelope<T>, bincode::Error> {
        bincode::deserialize(bytes)
    }

    // Helper to enforce schema-version compatibility on receive:
    pub fn decode_checked<T: for<'de> Deserialize<'de>>(bytes: &[u8])
        -> Result<T, anyhow::Error>
    {
        let env: Envelope<T> = decode(bytes)?;
        if env.schema_version != SCHEMA_VERSION {
            anyhow::bail!("schema version mismatch: got {}, want {}",
                env.schema_version, SCHEMA_VERSION);
        }
        Ok(env.payload)
    }

2. Add uuid to crates/nats-transport/Cargo.toml with features ["v4", "serde"].

3. Create docs/NATS_SUBJECTS.md:

    # NATS Subjects

    ## Hierarchy

    | Subject | Publishers | Consumers | Transport | Schema |
    |---|---|---|---|---|
    | `md.<venue>.<instrument>.book` | `md-ingest-<venue>` | FV service, strategy controller | NATS Core (latest-value) | `Envelope<MdFrame>` |
    | `fv.<symbol>` | `fair-value-service` | strategy controller | NATS Core | `Envelope<FairValueMessage>` |
    | `action.<venue>.<market>` | strategy controller | obird engine | JetStream work-queue | `Envelope<ActionMsg>` |
    | `order.<venue>.<market>.<oid>` | obird engine | strategy controller, position-service, audit | JetStream durable | `Envelope<OrderUpdate>` |
    | `engine.<venue>.<market>.health` | obird engine | dashboard, Grafana | NATS Core (1 Hz) | `Envelope<Health>` |

    ## Schema evolution

    `schema_version` is bumped on breaking change. Engine supports reading
    N and N-1 for one release window. After migration, drop N-1 support.

    ## Idempotency

    Every `ActionMsg` has an `action_id: UUID`. Engine stores last 10k seen
    action_ids in-memory. Duplicates return the cached `OrderId` without
    hitting the exchange.

4. Unit tests in schemas.rs: roundtrip encode/decode for each message type.
```

## Acceptance Criteria
- [ ] `cargo test -p nats-transport` passes the roundtrip tests
- [ ] `docs/NATS_SUBJECTS.md` documents every subject
- [ ] Schema version mismatch on decode produces a clear error

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T1
