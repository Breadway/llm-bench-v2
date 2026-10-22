#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub draw: usize,
    pub discard: usize,
    pub end: bool,
}
impl Action {
    pub fn new(draw: usize, discard: usize) -> Self {
        Self {
            draw,
            discard,
            end: false,
        }
    }
    pub const END: Self = Self {
        draw: 0,
        discard: 0,
        end: true,
    };
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhaseResult {
    pub hand_size: usize,
    pub actions: usize,
    pub forced_end: bool,
}
pub fn run_phase(
    mut hand_size: usize,
    hand_limit: usize,
    policy: &[Action],
    max_actions: usize,
) -> PhaseResult {
    let mut actions = 0;
    while hand_size > hand_limit {
        if actions == max_actions {
            return PhaseResult {
                hand_size,
                actions,
                forced_end: true,
            };
        }
        let Some(action) = policy.get(actions % policy.len().max(1)) else {
            return PhaseResult {
                hand_size,
                actions,
                forced_end: false,
            };
        };
        if policy.is_empty() || action.end {
            return PhaseResult {
                hand_size,
                actions,
                forced_end: false,
            };
        }
        hand_size = hand_size
            .saturating_add(action.draw)
            .saturating_sub(action.discard);
        actions += 1;
    }
    PhaseResult {
        hand_size,
        actions,
        forced_end: false,
    }
}
