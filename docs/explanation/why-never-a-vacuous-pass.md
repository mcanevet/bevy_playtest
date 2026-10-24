# Why never a vacuous pass

The worst failure mode of a test harness is a silent "green" that verified
nothing. So the harness rejects anything it can't actually check: unsupported
invariant kinds, unknown hook names, malformed scenarios all fail at load
with explicit errors. A passing report means the oracles genuinely ran.
