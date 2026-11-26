//! LocalAgreement-n confirmation policy (Macháček et al., arXiv:2307.14743).
//! Exact behavioral parity with `cronista/engine/local_agreement.py`.

pub fn tokenize(text: Option<&str>) -> Vec<String> {
    match text {
        Some(t) if !t.is_empty() => t.split_whitespace().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

pub fn common_prefix(token_lists: &[Vec<String>]) -> Vec<String> {
    if token_lists.is_empty() {
        return Vec::new();
    }
    let shortest = token_lists.iter().map(|v| v.len()).min().unwrap_or(0);
    let mut prefix = Vec::new();
    for i in 0..shortest {
        let tok = &token_lists[0][i];
        if token_lists.iter().all(|list| &list[i] == tok) {
            prefix.push(tok.clone());
        } else {
            break;
        }
    }
    prefix
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub committed: String,
    pub pending: String,
    pub newly_committed: String,
}

#[derive(Debug, Clone)]
pub struct LocalAgreement {
    n: usize,
    committed: Vec<String>,
    history: Vec<Vec<String>>,
}

impl LocalAgreement {
    pub fn new(n: usize) -> Result<Self, &'static str> {
        if n < 2 {
            return Err("LocalAgreement needs n >= 2");
        }
        Ok(Self {
            n,
            committed: Vec::new(),
            history: Vec::new(),
        })
    }

    pub fn default_n2() -> Self {
        Self::new(2).unwrap()
    }

    fn strip_committed(&self, tokens: Vec<String>) -> Vec<String> {
        if tokens.len() <= self.committed.len() {
            Vec::new()
        } else {
            tokens[self.committed.len()..].to_vec()
        }
    }

    fn snapshot(&self, pending: &[String], newly_committed: &[String]) -> Snapshot {
        Snapshot {
            committed: self.committed.join(" "),
            pending: pending.join(" "),
            newly_committed: newly_committed.join(" "),
        }
    }

    pub fn update(&mut self, text: Option<&str>) -> Snapshot {
        let rest = self.strip_committed(tokenize(text));
        self.history.push(rest);
        if self.history.len() > self.n {
            self.history.remove(0);
        }
        if self.history.len() < self.n {
            let pending = self.history.last().cloned().unwrap_or_default();
            return self.snapshot(&pending, &[]);
        }

        let agreed = common_prefix(&self.history);
        if !agreed.is_empty() {
            let cut = agreed.len();
            self.committed.extend(agreed.iter().cloned());
            for hyp in &mut self.history {
                hyp.drain(..cut);
            }
        }
        let pending = self.history.last().cloned().unwrap_or_default();
        self.snapshot(&pending, &agreed)
    }

    pub fn flush(&mut self, final_text: Option<&str>) -> Snapshot {
        let tail = match final_text {
            None => self.history.last().cloned().unwrap_or_default(),
            Some(ft) => self.strip_committed(tokenize(Some(ft))),
        };
        self.committed.extend(tail.iter().cloned());
        self.history.clear();
        self.snapshot(&[], &tail)
    }

    pub fn reset(&mut self) {
        self.committed.clear();
        self.history.clear();
    }

    pub fn text(&self) -> String {
        self.committed.join(" ")
    }
}
