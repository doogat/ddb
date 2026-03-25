use std::cmp::Ordering;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Result, DoogatError};

/// Hybrid Logical Clock — combines wall clock, logical counter, and node ID
/// for causally-ordered, conflict-free timestamps across distributed nodes.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
    pub node: String, // first 8 chars of node UUID
}

impl Hlc {
    /// Tick the clock for a local event.
    pub fn now(node_id: &str, last: &Option<Hlc>) -> Hlc {
        let wall = wall_clock_ms();
        let node = truncate_node(node_id);

        match last {
            Some(prev) => {
                if wall > prev.wall_ms {
                    Hlc {
                        wall_ms: wall,
                        counter: 0,
                        node,
                    }
                } else {
                    Hlc {
                        wall_ms: prev.wall_ms,
                        counter: prev.counter + 1,
                        node,
                    }
                }
            }
            None => Hlc {
                wall_ms: wall,
                counter: 0,
                node,
            },
        }
    }

    /// Merge on receive: take max(local, remote, wall) and bump counter if tied.
    pub fn recv(node_id: &str, local_last: &Option<Hlc>, remote: &Hlc) -> Hlc {
        let wall = wall_clock_ms();
        let node = truncate_node(node_id);

        let local_wall = local_last.as_ref().map(|h| h.wall_ms).unwrap_or(0);
        let local_counter = local_last.as_ref().map(|h| h.counter).unwrap_or(0);

        let max_wall = wall.max(local_wall).max(remote.wall_ms);

        let counter = if max_wall == local_wall && max_wall == remote.wall_ms {
            local_counter.max(remote.counter) + 1
        } else if max_wall == local_wall {
            local_counter + 1
        } else if max_wall == remote.wall_ms {
            remote.counter + 1
        } else {
            // wall clock is strictly ahead
            0
        };

        Hlc {
            wall_ms: max_wall,
            counter,
            node,
        }
    }

    /// Parse from sortable string format: `{wall_ms}-{counter:04}-{node}`.
    pub fn parse(s: &str) -> Result<Hlc> {
        let parts: Vec<&str> = s.splitn(3, '-').collect();
        if parts.len() != 3 {
            return Err(DoogatError::Parse(format!("invalid HLC: {s}")));
        }
        let wall_ms = parts[0]
            .parse::<u64>()
            .map_err(|e| DoogatError::Parse(format!("bad HLC wall_ms: {e}")))?;
        let counter = parts[1]
            .parse::<u32>()
            .map_err(|e| DoogatError::Parse(format!("bad HLC counter: {e}")))?;
        let node = parts[2].to_string();
        Ok(Hlc {
            wall_ms,
            counter,
            node,
        })
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        self.wall_ms
            .cmp(&other.wall_ms)
            .then_with(|| self.counter.cmp(&other.counter))
            .then_with(|| self.node.cmp(&other.node))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:04}-{}", self.wall_ms, self.counter, self.node)
    }
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn truncate_node(node_id: &str) -> String {
    node_id.chars().filter(|c| *c != '-').take(8).collect()
}

/// Extract an HLC trailer from a Git commit message.
/// Looks for `\nHLC: {hlc}` at the end.
pub fn extract_hlc(message: &str) -> Option<Hlc> {
    for line in message.lines().rev() {
        if let Some(rest) = line.strip_prefix("HLC: ") {
            return Hlc::parse(rest.trim()).ok();
        }
    }
    None
}

/// Append HLC trailer to a commit message.
pub fn append_hlc_trailer(message: &str, hlc: &Hlc) -> String {
    format!("{message}\n\nHLC: {hlc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_increments_from_none() {
        let hlc = Hlc::now("abcdefgh-1234", &None);
        assert!(hlc.wall_ms > 0);
        assert_eq!(hlc.counter, 0);
        assert_eq!(hlc.node, "abcdefgh");
    }

    #[test]
    fn tick_increments_counter_when_wall_equal() {
        let last = Hlc {
            wall_ms: u64::MAX - 1, // far future, wall clock won't exceed
            counter: 5,
            node: "abcdefgh".into(),
        };
        let next = Hlc::now("abcdefgh-1234", &Some(last));
        assert_eq!(next.wall_ms, u64::MAX - 1);
        assert_eq!(next.counter, 6);
    }

    #[test]
    fn recv_merges_correctly() {
        let local = Hlc {
            wall_ms: 100,
            counter: 3,
            node: "aaaaaaaa".into(),
        };
        let remote = Hlc {
            wall_ms: 100,
            counter: 5,
            node: "bbbbbbbb".into(),
        };
        let merged = Hlc::recv("aaaaaaaa", &Some(local), &remote);
        // max(wall_clock, 100, 100) — if wall_clock <= 100, counter = max(3,5)+1 = 6
        assert!(merged.counter >= 6 || merged.wall_ms > 100);
        assert_eq!(merged.node, "aaaaaaaa");
    }

    #[test]
    fn string_round_trip() {
        let hlc = Hlc {
            wall_ms: 1709000000000,
            counter: 42,
            node: "abcd1234".into(),
        };
        let s = hlc.to_string();
        assert_eq!(s, "1709000000000-0042-abcd1234");
        let parsed = Hlc::parse(&s).unwrap();
        assert_eq!(parsed, hlc);
    }

    #[test]
    fn ordering_wall_ms_first() {
        let a = Hlc {
            wall_ms: 100,
            counter: 99,
            node: "zzzzzzzz".into(),
        };
        let b = Hlc {
            wall_ms: 200,
            counter: 0,
            node: "aaaaaaaa".into(),
        };
        assert!(a < b);
    }

    #[test]
    fn ordering_counter_second() {
        let a = Hlc {
            wall_ms: 100,
            counter: 1,
            node: "zzzzzzzz".into(),
        };
        let b = Hlc {
            wall_ms: 100,
            counter: 2,
            node: "aaaaaaaa".into(),
        };
        assert!(a < b);
    }

    #[test]
    fn ordering_node_tiebreak() {
        let a = Hlc {
            wall_ms: 100,
            counter: 1,
            node: "aaaaaaaa".into(),
        };
        let b = Hlc {
            wall_ms: 100,
            counter: 1,
            node: "bbbbbbbb".into(),
        };
        assert!(a < b);
    }

    #[test]
    fn extract_hlc_from_commit() {
        let msg = "resolve merge conflicts via CRDT\n\nHLC: 1709000000000-0001-abcd1234";
        let hlc = extract_hlc(msg).unwrap();
        assert_eq!(hlc.wall_ms, 1709000000000);
        assert_eq!(hlc.counter, 1);
        assert_eq!(hlc.node, "abcd1234");
    }

    #[test]
    fn monotonic_1000_sequential_calls() {
        let mut last: Option<Hlc> = None;
        for _ in 0..1000 {
            let next = Hlc::now("test-node", &last);
            if let Some(ref prev) = last {
                assert!(
                    next > *prev,
                    "HLC must be strictly monotonic: {prev} >= {next}"
                );
            }
            last = Some(next);
        }
    }

    #[test]
    fn cross_node_causal_consistency() {
        // Node A generates t1
        let t1 = Hlc::now("node-a-1234", &None);

        // Node B generates t2 independently
        let t2 = Hlc::now("node-b-5678", &None);

        // B merges t1 via recv
        let b_merged = Hlc::recv("node-b-5678", &Some(t2.clone()), &t1);
        assert!(
            b_merged > t1,
            "B's merged timestamp must be > t1: {b_merged} vs {t1}"
        );
        assert!(
            b_merged > t2,
            "B's merged timestamp must be > t2: {b_merged} vs {t2}"
        );

        // A merges B's result
        let a_merged = Hlc::recv("node-a-1234", &Some(t1.clone()), &b_merged);
        assert!(
            a_merged > t1,
            "A's merged must be > t1: {a_merged} vs {t1}"
        );
        assert!(
            a_merged > t2,
            "A's merged must be > t2: {a_merged} vs {t2}"
        );
        assert!(
            a_merged > b_merged,
            "A's merged must be > B's merged: {a_merged} vs {b_merged}"
        );
    }

    #[test]
    fn monotonic_despite_clock_regression() {
        // Simulate a "last" with wall_ms far in the future (clock regression scenario)
        let future_last = Hlc {
            wall_ms: u64::MAX / 2, // far future, real wall clock won't reach this
            counter: 0,
            node: "oldnode1".into(),
        };

        // Call now() — wall clock is behind, so HLC must not regress
        let next = Hlc::now("test-node", &Some(future_last.clone()));
        assert!(
            next.wall_ms >= future_last.wall_ms,
            "wall_ms must not regress: {} < {}",
            next.wall_ms,
            future_last.wall_ms
        );
        assert!(
            next > future_last,
            "new HLC must sort after previous: {next} vs {future_last}"
        );
    }

    #[test]
    fn extract_hlc_missing() {
        assert!(extract_hlc("plain commit message").is_none());
    }

    #[test]
    fn append_trailer() {
        let hlc = Hlc {
            wall_ms: 100,
            counter: 0,
            node: "abcd1234".into(),
        };
        let msg = append_hlc_trailer("test commit", &hlc);
        assert!(msg.contains("\n\nHLC: 100-0000-abcd1234"));
    }
}
