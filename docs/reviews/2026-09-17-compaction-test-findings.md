# Follow-ups from compaction testing

Status: confirmed existing defects; separately scoped fixes recommended. Neither
is fixed by the context-budget feature.

## Unrecognized notifications while waiting

An unrecognized provider notification received while a run is waiting for user
input can fail persistence. `stage_waiting_event_inner` in
`crates/prompting-time-core/src/store.rs` stages the record as a diagnostic without
overflow metadata, but migration `0006_bound_staged_provider_events.sql` permits
diagnostics only as overflow markers. A focused store reproduction entering an
owned waiting state and staging `ProviderEventRecord::unrecognized(...)` failed
with SQLite error 275 (CHECK constraint). This is storage-boundary evidence, not a
claim of a reproduced authenticated-provider failure.

Compaction observations use a permitted typed progress envelope while staged and
are projected on drain. The redundant, deprecated Codex `thread/compacted`
notification is suppressed in favor of the canonical compaction item lifecycle.
Neither change resolves the general unrecognized-notification path.

A focused follow-up should reconcile the queue schema and diagnostic record
contract, preserve ownership and queue bounds, and add a regression that stages
and drains an unknown notification without failing an otherwise valid run.

## Claude health-check error classification

The Claude adapter reports version-inspection failures with the same diagnostic
as a missing executable; application routing maps that diagnostic to `NotInstalled`.
A temporary synthetic probe confirmed that an existing executable's inspection
timeout can receive this classification. A focused follow-up should distinguish
actual spawn `NotFound` errors from other inspection failures, with health/routing
regressions. Correcting the label would not resolve the timeout itself.

The underlying startup delay remains unconfirmed: three affected app tests passed
on a fresh integrated build, but a minimal direct-health probe reproduced the delay
without opening a database, and an older unchanged binary failed then passed. This
does not establish a compaction regression, an app-only cause, or a fresh-build fix.
No production timeout was increased and no native model call was needed to investigate.
