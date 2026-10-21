//! Parallel test scheduling.
//!
//! A test may declare conflict keys (a `--CONFLICTS--` section, or a
//! `CONFLICTS` file in its directory).  While a test holding key `K` runs, no
//! other test holding `K` may start.  Tests that conflict with `all` run
//! alone, after everything else.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// `parse_conflicts()`: strip `#` comments, trim, drop empty lines.
pub fn parse_conflicts(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Extract the `--CONFLICTS--` section of a `.phpt` file, if it has one.
///
/// Mirrors the upstream pattern `/^--CONFLICTS--(.+?)^--/ms`: the section
/// must be followed by another `--` line.
pub fn conflicts_section(contents: &[u8]) -> Option<String> {
    let mut lines = contents.split_inclusive(|&b| b == b'\n');
    lines.find(|l| l.starts_with(b"--CONFLICTS--"))?;
    let mut body = Vec::new();
    for line in lines {
        if line.starts_with(b"--") {
            return Some(String::from_utf8_lossy(&body).into_owned());
        }
        body.extend_from_slice(line);
    }
    None
}

/// Conflict keys of every test, reading each file once and caching
/// directory-level `CONFLICTS` files.
pub fn collect_conflicts(tests: &[PathBuf]) -> Vec<Vec<String>> {
    let mut dir_cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
    tests
        .iter()
        .map(|path| {
            if let Ok(contents) = std::fs::read(path)
                && let Some(section) = conflicts_section(&contents)
            {
                return parse_conflicts(&section);
            }
            let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            dir_cache
                .entry(dir.clone())
                .or_insert_with(|| {
                    std::fs::read_to_string(dir.join("CONFLICTS"))
                        .map(|t| parse_conflicts(&t))
                        .unwrap_or_default()
                })
                .clone()
        })
        .collect()
}

struct State {
    pending: VecDeque<usize>,
    sequential: VecDeque<usize>,
    active: HashSet<String>,
    running: usize,
    exclusive: bool,
}

/// Hands out test indices to worker threads.
pub struct Scheduler {
    conflicts: Vec<Vec<String>>,
    state: Mutex<State>,
    wake: Condvar,
}

/// A claimed test; pass it back to [`Scheduler::finish`].
#[derive(Debug)]
pub struct Ticket {
    pub index: usize,
    exclusive: bool,
}

impl Scheduler {
    /// `conflicts[i]` are the keys of test `i`; tests are started in index order.
    pub fn new(conflicts: Vec<Vec<String>>) -> Self {
        let mut pending = VecDeque::new();
        let mut sequential = VecDeque::new();
        for (i, keys) in conflicts.iter().enumerate() {
            if keys.iter().any(|k| k == "all") {
                sequential.push_back(i);
            } else {
                pending.push_back(i);
            }
        }
        Self {
            conflicts,
            state: Mutex::new(State {
                pending,
                sequential,
                active: HashSet::new(),
                running: 0,
                exclusive: false,
            }),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claim the next runnable test, blocking while conflicts or an exclusive
    /// test prevent progress.  Returns `None` once everything has been handed out.
    pub fn next(&self) -> Option<Ticket> {
        let mut st = self.lock();
        loop {
            if st.pending.is_empty() && st.sequential.is_empty() {
                return None;
            }
            if !st.exclusive {
                let pick = st
                    .pending
                    .iter()
                    .position(|&i| self.conflicts[i].iter().all(|k| !st.active.contains(k)));
                if let Some(pos) = pick {
                    let index = st.pending.remove(pos).unwrap_or_default();
                    st.active.extend(self.conflicts[index].iter().cloned());
                    st.running += 1;
                    return Some(Ticket {
                        index,
                        exclusive: false,
                    });
                }
                if st.pending.is_empty() && st.running == 0 {
                    let index = st.sequential.pop_front()?;
                    st.exclusive = true;
                    st.running += 1;
                    return Some(Ticket {
                        index,
                        exclusive: true,
                    });
                }
            }
            st = self.wake.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Drop every test that has not started yet (used on interrupt).
    pub fn cancel(&self) {
        let mut st = self.lock();
        st.pending.clear();
        st.sequential.clear();
        drop(st);
        self.wake.notify_all();
    }

    /// Release the keys held by a finished test.
    pub fn finish(&self, ticket: Ticket) {
        let mut st = self.lock();
        for key in &self.conflicts[ticket.index] {
            st.active.remove(key);
        }
        st.running -= 1;
        if ticket.exclusive {
            st.exclusive = false;
        }
        drop(st);
        self.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn parse_conflicts_strips_comments_and_blanks() {
        assert_eq!(
            parse_conflicts("# top\nserver # inline\n\n  db  \n"),
            vec!["server", "db"]
        );
        assert!(parse_conflicts("").is_empty());
    }

    #[test]
    fn conflicts_section_requires_a_following_marker() {
        let t = b"--TEST--\nx\n--CONFLICTS--\nfoo\nbar\n--FILE--\nx\n";
        assert_eq!(conflicts_section(t).as_deref(), Some("foo\nbar\n"));
        let at_end = b"--TEST--\nx\n--CONFLICTS--\nfoo\n";
        assert_eq!(conflicts_section(at_end), None);
        assert_eq!(conflicts_section(b"--TEST--\nx\n--FILE--\ny\n"), None);
    }

    #[test]
    fn collects_file_and_directory_conflicts() {
        let dir = std::env::temp_dir().join(format!("php-phpt-sched-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("CONFLICTS"), "dirkey # c\n").unwrap();
        std::fs::write(
            dir.join("a.phpt"),
            "--TEST--\na\n--CONFLICTS--\nfilekey\n--FILE--\nx\n--EXPECT--\nx\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.phpt"),
            "--TEST--\nb\n--FILE--\nx\n--EXPECT--\nx\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("sub/c.phpt"),
            "--TEST--\nc\n--FILE--\nx\n--EXPECT--\nx\n",
        )
        .unwrap();
        let got = collect_conflicts(&[
            dir.join("a.phpt"),
            dir.join("b.phpt"),
            dir.join("sub/c.phpt"),
        ]);
        let expected: Vec<Vec<String>> =
            vec![vec!["filekey".into()], vec!["dirkey".into()], vec![]];
        assert_eq!(got, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn drain(s: &Scheduler) -> Vec<usize> {
        let mut order = Vec::new();
        while let Some(t) = s.next() {
            order.push(t.index);
            s.finish(t);
        }
        order
    }

    #[test]
    fn single_worker_preserves_order_with_sequential_tests_last() {
        let s = Scheduler::new(vec![
            vec![],
            vec!["all".into()],
            vec!["k".into()],
            vec![],
            vec!["all".into()],
        ]);
        assert_eq!(drain(&s), vec![0, 2, 3, 1, 4]);
    }

    #[test]
    fn conflicting_tests_never_overlap() {
        let conflicts: Vec<Vec<String>> = (0..40)
            .map(|i| {
                if i % 2 == 0 {
                    vec!["db".to_string()]
                } else {
                    vec![]
                }
            })
            .collect();
        let s = Arc::new(Scheduler::new(conflicts));
        let in_db = Arc::new(AtomicUsize::new(0));
        let max_db = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let (s, in_db, max_db, done) =
                    (s.clone(), in_db.clone(), max_db.clone(), done.clone());
                thread::spawn(move || {
                    while let Some(t) = s.next() {
                        if t.index % 2 == 0 {
                            let now = in_db.fetch_add(1, Ordering::SeqCst) + 1;
                            max_db.fetch_max(now, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(2));
                            in_db.fetch_sub(1, Ordering::SeqCst);
                        } else {
                            thread::sleep(Duration::from_millis(1));
                        }
                        done.fetch_add(1, Ordering::SeqCst);
                        s.finish(t);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(done.load(Ordering::SeqCst), 40);
        assert_eq!(max_db.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn all_conflict_runs_alone_after_the_rest() {
        let mut conflicts: Vec<Vec<String>> = vec![vec![]; 12];
        conflicts[3] = vec!["all".into()];
        let s = Arc::new(Scheduler::new(conflicts));
        let running = Arc::new(AtomicUsize::new(0));
        let finished_normal = Arc::new(AtomicUsize::new(0));
        let violations = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (s, running, fin, bad) = (
                    s.clone(),
                    running.clone(),
                    finished_normal.clone(),
                    violations.clone(),
                );
                thread::spawn(move || {
                    while let Some(t) = s.next() {
                        let before = running.fetch_add(1, Ordering::SeqCst);
                        if t.index == 3 && (before != 0 || fin.load(Ordering::SeqCst) != 11) {
                            bad.fetch_add(1, Ordering::SeqCst);
                        }
                        thread::sleep(Duration::from_millis(3));
                        if t.index != 3 {
                            fin.fetch_add(1, Ordering::SeqCst);
                        }
                        running.fetch_sub(1, Ordering::SeqCst);
                        s.finish(t);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(violations.load(Ordering::SeqCst), 0);
    }
}
