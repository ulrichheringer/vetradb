# ADR 0004: Relational versions and immutable transaction basis

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Preserve a relational SQL model with stable internal row/table/column/schema IDs and immutable committed mutation envelopes. Commit sequence number, scoped to database/timeline, is the precise historical ordering key. AS OF pins data and schema to one basis; history exposes all retained operations, including retractions and provenance.

## Alternatives and consequences

A Datomic-like universal datom model would change SQL/type/constraint ergonomics and does not follow from the requirement for Datomic-inspired history. Application audit triggers would miss engine/service mutations and could diverge on failure.

Primary-key changes do not replace row identity. Timestamp AS OF has a documented tie/skew resolution and cannot be as precise as a CSN. Business valid time remains an application field; native bitemporal semantics need separate design. Historical schema retention and lossless values add storage cost that benchmarks must measure.
