//! J0: ChoiceStream — record and replay bot random choices.
//!
//! Bots draw from this stream instead of a raw PRNG. Draws come from
//! `replay` while it lasts, then from `continuation` (default: PRNG).
//! Every draw is RECORDED with frame, reduced value and bound.

use serde::{Deserialize, Serialize};

/// Continuation semantics once replayed choices run out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Continuation {
    /// Keep drawing from the seeded PRNG (default).
    #[default]
    Prng,
    /// Draw zeros (simplest choices).
    Zeros,
    /// Reseed the PRNG with a fixed value.
    Reseed(u64),
}

/// What a recorded draw was for. Downstream report consumers (action
/// log, key-surface coverage) must not conflate mouse draws with key
/// draws — a mouse button-index draw would masquerade as a key press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceKind {
    /// A chaos-bot key-index draw.
    #[default]
    Key,
    /// A chaos-bot cursor-position draw.
    MousePosition,
    /// A chaos-bot mouse button-index draw.
    MouseButton,
    /// A chaos-bot per-press key hold-duration draw. The recorded value
    /// is reduced in `0..=28`; the delivered hold is `value + 2`
    /// frames. Deliberately NOT [`ChoiceKind::Key`]: the action log
    /// filters on `Key`, and a duration draw is not a key press.
    KeyHoldFrames,
}

/// One recorded random draw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub frame: u64,
    /// Reduced value in [0, bound).
    pub value: u64,
    /// 0 = raw u64 draw (no reduction).
    pub bound: u64,
    /// What the draw selected — lets report consumers separate key
    /// draws (action log, surface coverage) from mouse draws.
    #[serde(default)]
    pub kind: ChoiceKind,
    /// What the reduced value RESOLVED to, when that is meaningful:
    /// a key-index draw records the canonical key name it selected.
    /// Without this, `value` would index a draw-time list (e.g. the
    /// unheld-key candidates) that the report does not contain, and
    /// the action log could not name the pressed key. Absent (None,
    /// serialized only when set) for draws whose value is not an
    /// index — mouse coordinates, hold durations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Source of bot randomness (plain struct, no Bevy Resource derive).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceStream {
    rng: u64,
    replay: std::collections::VecDeque<Choice>,
    pub continuation: Continuation,
    reseeded: bool,
    /// Every draw this run.
    pub recorded: Vec<Choice>,
}

impl Default for ChoiceStream {
    fn default() -> Self {
        Self::from_seed(42)
    }
}

impl ChoiceStream {
    /// New stream from a scenario seed (seed 0 maps to 42).
    pub fn from_seed(seed: u64) -> Self {
        Self {
            rng: if seed == 0 { 42 } else { seed },
            replay: Default::default(),
            continuation: Continuation::Prng,
            reseeded: false,
            recorded: Vec::new(),
        }
    }

    /// Install a replay prefix and continuation.
    pub fn with_replay(mut self, choices: Vec<Choice>, continuation: Continuation) -> Self {
        self.replay = choices.into();
        self.continuation = continuation;
        self
    }

    /// The next raw u64 from xorshift64*.
    fn next_raw(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// Uniform in [0, bound). Records the REDUCED value.
    ///
    /// Replay entries are FRAME-INDEXED: a draw at frame F serves the
    /// entry recorded for frame F. Entries recorded for frames that
    /// were skipped (never drawn) are consumed without effect — they
    /// can no longer shift later draws onto the wrong values.
    ///
    /// Deliberately no untagged variant: the action log filters on
    /// `ChoiceKind::Key`, so an implicitly-`Key`-tagged helper would
    /// let a future caller fabricate `key_press` entries. Call
    /// [`below_kind`](Self::below_kind) with an explicit kind.
    pub fn below_kind(&mut self, frame: u64, bound: u64, kind: ChoiceKind) -> u64 {
        // Consume replay entries older than the requested frame.
        while self.replay.front().is_some_and(|c| c.frame < frame) {
            self.replay.pop_front();
        }
        let replay_value = if self.replay.front().is_some_and(|c| c.frame == frame) {
            self.replay.pop_front().map(|c| c.value)
        } else {
            None
        };
        let value = match replay_value {
            Some(v) => v % bound.max(1),
            None => {
                if bound <= 1 {
                    0
                } else {
                    match self.continuation {
                        Continuation::Prng => {
                            (((self.next_raw() as u128) * (bound as u128)) >> 64) as u64
                        }
                        Continuation::Zeros => 0,
                        Continuation::Reseed(seed) => {
                            if !self.reseeded {
                                self.rng = if seed == 0 { 42 } else { seed };
                                self.reseeded = true;
                            }
                            (((self.next_raw() as u128) * (bound as u128)) >> 64) as u64
                        }
                    }
                }
            }
        };
        self.recorded.push(Choice {
            frame,
            value,
            bound,
            kind,
            label: None,
        });
        value
    }

    /// Attach `label` to the most recently recorded draw. Used when a
    /// draw's reduced value is an index into a draw-time list (e.g. the
    /// chaos bot's unheld-key candidates): the resolution depends on
    /// the drawn value, so the label can only be computed AFTER the
    /// draw — the caller resolves the index and stamps it here.
    /// Without the label, the action log could not name the pressed
    /// key from the report alone.
    pub fn label_last(&mut self, label: String) {
        if let Some(last) = self.recorded.last_mut() {
            last.label = Some(label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_consumes_correctly() {
        let mut cs = ChoiceStream::from_seed(42).with_replay(
            vec![
                Choice {
                    frame: 1,
                    value: 0,
                    bound: 1,
                    kind: ChoiceKind::Key,
                    label: None,
                },
                Choice {
                    frame: 2,
                    value: 5,
                    bound: 10,
                    kind: ChoiceKind::Key,
                    label: None,
                },
                Choice {
                    frame: 3,
                    value: 3,
                    bound: 4,
                    kind: ChoiceKind::Key,
                    label: None,
                },
            ],
            Continuation::Prng,
        );
        let a = cs.below_kind(1, 1, ChoiceKind::Key);
        let b = cs.below_kind(2, 10, ChoiceKind::Key);
        let c = cs.below_kind(3, 4, ChoiceKind::Key);
        assert_eq!(a, 0);
        assert_eq!(b, 5);
        assert_eq!(c, 3);
    }

    #[test]
    fn prng_is_deterministic() {
        let mut cs1 = ChoiceStream::from_seed(123);
        let mut cs2 = ChoiceStream::from_seed(123);
        for i in 0..10 {
            assert_eq!(
                cs1.below_kind(i, 100, ChoiceKind::Key),
                cs2.below_kind(i, 100, ChoiceKind::Key)
            );
        }
    }
}
