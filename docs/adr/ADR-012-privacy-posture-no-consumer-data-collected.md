# ADR-012: Privacy Posture — No Consumer Data Collected

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB stores agent experiences locally. The privacy posture must be clear.

## Decision
PulseDB collects no consumer data. All data stored in PulseDB databases belongs to the consumer application and its users. PulseDB itself has no telemetry, analytics, or phone-home. Privacy is the consumer's responsibility — they control what experiences are recorded. The AI workspace (planning, specs, memory bank) is private and never committed to the public canonical repository.

## Touch surface
`README.md`, `CLAUDE.md`, `PUBLIC_BOUNDARY.md`

## Revisit trigger
Not applicable — this is the core privacy posture of PulseDB.
