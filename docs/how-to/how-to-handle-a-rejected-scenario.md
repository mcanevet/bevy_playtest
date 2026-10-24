# Handle a rejected scenario

Scenario load fails loudly rather than passing vacuously. The complete
rejection list — unsupported knobs (`NotImplemented`: `custom` invariants,
`eventually` mode, target filtering, time windows), invalid values
(negative durations, broken bound pairs, typo'd key names), unknown
fields — is in the [reference](../reference/#scenario-json-schema).
