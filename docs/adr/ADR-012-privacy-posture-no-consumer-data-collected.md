# ADR-012: Privacy Posture — No Consumer Data Collected

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB stores agent experiences locally. The privacy posture must be clear.

## Decision
PulseDB collects no consumer data. All data stored in PulseDB databases belongs to the consumer application and its users. PulseDB itself has no telemetry, analytics, or phone-home. **One outbound-connection exception:** with `builtin-embeddings` enabled and an empty model cache, the first use auto-downloads the ONNX model + tokenizer from Hugging Face over HTTPS (no database content is transmitted; cache the models to run fully network-free). Privacy is the consumer's responsibility — they control what experiences are recorded. The AI workspace (planning, specs, memory bank) is private and never committed to the public canonical repository.

## Touch surface
`README.md`, `PUBLIC_BOUNDARY.md`, `SECURITY.md`

## Revisit trigger
Not applicable — this is the core privacy posture of PulseDB.
