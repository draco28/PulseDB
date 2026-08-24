# ADR-012: Privacy Posture — No Consumer Data Collected

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB stores agent experiences locally. The privacy posture must be clear.

## Decision
PulseDB collects no consumer data. All data stored in PulseDB databases belongs to the consumer application and its users. PulseDB itself has no telemetry, analytics, or phone-home. **Two consumer-configured outbound paths, no telemetry:** (1) with `builtin-embeddings` enabled and an empty model cache, the first use auto-downloads the ONNX model + tokenizer from Hugging Face over HTTPS (no database content transmitted; cache the models to run network-free); (2) with `sync-http` enabled, stored entities are serialized and pushed to the configured peer — that egress transmits database *content* by design and is entirely consumer-configured (never automatic). Privacy is the consumer's responsibility — they control what experiences are recorded. The AI workspace (planning, specs, memory bank) is private and never committed to the public canonical repository.

## Touch surface
`README.md`, `PUBLIC_BOUNDARY.md`, `SECURITY.md`

## Revisit trigger
Not applicable — this is the core privacy posture of PulseDB.
