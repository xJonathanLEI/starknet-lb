use std::cmp::Ordering;

#[derive(Debug, Clone, Copy)]
pub enum ChainHead {
    Confirmed(ConfirmedHead),
    Pending(PendingHead),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmedHead {
    pub height: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingHead {
    pub height: u64,
    pub tx_count: usize,
}

impl Eq for ChainHead {}

impl PartialEq<ChainHead> for ChainHead {
    fn eq(&self, other: &ChainHead) -> bool {
        match (self, other) {
            (Self::Confirmed(left), Self::Confirmed(right)) => left == right,
            (Self::Confirmed(left), Self::Pending(right)) => left == right,
            (Self::Pending(left), Self::Confirmed(right)) => left == right,
            (Self::Pending(left), Self::Pending(right)) => left == right,
        }
    }
}

impl Ord for ChainHead {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Confirmed(left), Self::Confirmed(right)) => left.cmp(right),
            // Workaround for not having `Ord` between types. This is safe. See:
            //
            // https://users.rust-lang.org/t/implement-ord-to-compare-different-types/73106
            (Self::Confirmed(left), Self::Pending(right)) => unsafe {
                left.partial_cmp(right).unwrap_unchecked()
            },
            // Workaround for not having `Ord` between types. This is safe. See:
            //
            // https://users.rust-lang.org/t/implement-ord-to-compare-different-types/73106
            (Self::Pending(left), Self::Confirmed(right)) => unsafe {
                left.partial_cmp(right).unwrap_unchecked()
            },
            (Self::Pending(left), Self::Pending(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd<ChainHead> for ChainHead {
    fn partial_cmp(&self, other: &ChainHead) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq<ConfirmedHead> for PendingHead {
    fn eq(&self, other: &ConfirmedHead) -> bool {
        self.tx_count == 0 && self.height == other.height + 1
    }
}

impl PartialEq<PendingHead> for ConfirmedHead {
    fn eq(&self, other: &PendingHead) -> bool {
        other.tx_count == 0 && other.height == self.height + 1
    }
}

impl Ord for ConfirmedHead {
    fn cmp(&self, other: &Self) -> Ordering {
        self.height.cmp(&other.height)
    }
}

impl PartialOrd<ConfirmedHead> for ConfirmedHead {
    fn partial_cmp(&self, other: &ConfirmedHead) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingHead {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.height.cmp(&other.height) {
            Ordering::Equal => self.tx_count.cmp(&other.tx_count),
            ordering => ordering,
        }
    }
}

impl PartialOrd<PendingHead> for PendingHead {
    fn partial_cmp(&self, other: &PendingHead) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialOrd<ConfirmedHead> for PendingHead {
    fn partial_cmp(&self, other: &ConfirmedHead) -> Option<Ordering> {
        Some(match self.height.cmp(&(other.height + 1)) {
            Ordering::Equal => {
                if self.tx_count == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                }
            }
            ordering => ordering,
        })
    }
}

impl PartialOrd<PendingHead> for ConfirmedHead {
    fn partial_cmp(&self, other: &PendingHead) -> Option<Ordering> {
        Some(match (self.height + 1).cmp(&other.height) {
            Ordering::Equal => {
                if other.tx_count == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Less
                }
            }
            ordering => ordering,
        })
    }
}
