/// Connection-level transaction state. Tracked heuristically from the SQL we
/// run: tokio-postgres discards the protocol's ReadyForQuery status byte, so we
/// can't read it authoritatively. Advisory — drives the status-bar badge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxStatus {
    /// Not in a transaction block (autocommit).
    Idle = 0,
    /// Inside an open transaction — BEGIN with no COMMIT/ROLLBACK yet.
    Active = 1,
    /// Transaction aborted by an error — every statement errors until ROLLBACK.
    Failed = 2,
}

impl TxStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => TxStatus::Active,
            2 => TxStatus::Failed,
            _ => TxStatus::Idle,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TxStatus::Idle => "idle",
            TxStatus::Active => "active",
            TxStatus::Failed => "failed",
        }
    }
    /// Label of the tx state stored as a u8 (see [`Session::tx`]) — folds the
    /// `from_u8(x).as_str()` chain repeated at every session-info projection.
    pub fn label_from_u8(v: u8) -> &'static str {
        Self::from_u8(v).as_str()
    }
}

/// Splits multi-statement SQL on `;`, honoring '…'/E'…' strings, "…"
/// identifiers, $tag$…$tag$ dollar quoting and --/nested /* */ comments.
/// Statements that are only whitespace/comments are dropped — the simple-query
/// protocol yields no result for them, keeping this 1:1 with [`execute`].
pub fn split_statements(sql: &str) -> Vec<String> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0; // byte offset where the current statement begins
    let mut has_token = false; // current statement has non-comment content
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 1;
                i += 2;
                while i < b.len() && depth > 0 {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'\'' => {
                // E'…' also understands backslash escapes
                let estring = i > 0
                    && (b[i - 1] == b'e' || b[i - 1] == b'E')
                    && (i < 2 || !(b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_'));
                has_token = true;
                i += 1;
                while i < b.len() {
                    if estring && b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            i += 2; // '' — escaped quote
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'"' => {
                has_token = true;
                i += 1;
                while i < b.len() {
                    if b[i] == b'"' {
                        if b.get(i + 1) == Some(&b'"') {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                // $tag$ … $tag$; $1 (a digit after $) is a parameter, not a tag
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let is_tag = j < b.len()
                    && b[j] == b'$'
                    && (j == i + 1 || !b[i + 1].is_ascii_digit());
                has_token = true;
                if is_tag {
                    let tag = &b[i..=j];
                    i = j + 1;
                    while i + tag.len() <= b.len() && &b[i..i + tag.len()] != tag {
                        i += 1;
                    }
                    i = (i + tag.len()).min(b.len());
                } else {
                    i += 1;
                }
            }
            b';' => {
                if has_token {
                    out.push(sql[start..i].trim().to_string());
                }
                start = i + 1;
                has_token = false;
                i += 1;
            }
            c => {
                if !c.is_ascii_whitespace() {
                    has_token = true;
                }
                i += 1;
            }
        }
    }
    if has_token {
        out.push(sql[start..].trim().to_string());
    }
    out
}

/// Skips leading whitespace and SQL comments so keyword sniffing sees the
/// actual command (`-- note\nBEGIN` -> `BEGIN`).
fn strip_leading_noise(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |(_, r)| r);
        } else if s.starts_with("/*") {
            match s.find("*/") {
                Some(i) => s = &s[i + 2..],
                None => return "",
            }
        } else {
            return s;
        }
    }
}

/// The first two SQL keywords of a statement, uppercased (command + qualifier).
fn head_keywords(stmt: &str) -> (String, String) {
    let mut it = strip_leading_noise(stmt)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_uppercase());
    (it.next().unwrap_or_default(), it.next().unwrap_or_default())
}

/// Advances the tracked transaction status after running `sql` (which produced
/// `ok`). On success the outcome is exact — the whole batch ran (simple_query
/// returns Ok only then), so we fold the transaction verbs over its statements.
/// On failure we can't tell which statement broke, so we err toward "aborted"
/// (the safe nudge to ROLLBACK). Heuristic: exotic cases (COMMIT inside a
/// procedure, a partial multi-transaction batch) may mis-track.
pub fn advance_tx(cur: TxStatus, sql: &str, ok: bool) -> TxStatus {
    if !ok {
        let opens_tx = || {
            split_statements(sql)
                .iter()
                .any(|s| matches!(head_keywords(s).0.as_str(), "BEGIN" | "START"))
        };
        return match cur {
            // an implicit tx rolls back to idle; an explicit BEGIN that failed
            // partway leaves the connection in an aborted, still-open tx
            TxStatus::Idle if opens_tx() => TxStatus::Failed,
            TxStatus::Idle => TxStatus::Idle,
            _ => TxStatus::Failed,
        };
    }
    let mut s = cur;
    for stmt in split_statements(sql) {
        let (w0, w1) = head_keywords(&stmt);
        s = match w0.as_str() {
            "BEGIN" | "START" => TxStatus::Active,
            "COMMIT" | "END" | "ABORT" => TxStatus::Idle,
            // ROLLBACK TO [SAVEPOINT] keeps the tx open (and un-aborts it);
            // plain ROLLBACK ends it
            "ROLLBACK" if w1 == "TO" => {
                if s == TxStatus::Failed {
                    TxStatus::Active
                } else {
                    s
                }
            }
            "ROLLBACK" => TxStatus::Idle,
            _ => s,
        };
    }
    s
}

#[cfg(test)]
mod tests {
    use super::split_statements;

    #[test]
    fn splits_plain_statements() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;"),
            vec!["SELECT 1", "SELECT 2"]
        );
        // trailing statement without ';'
        assert_eq!(split_statements("SELECT 1"), vec!["SELECT 1"]);
    }

    #[test]
    fn semicolons_inside_quoting_do_not_split() {
        assert_eq!(
            split_statements("SELECT 'a;b'; SELECT \";\""),
            vec!["SELECT 'a;b'", "SELECT \";\""]
        );
        // '' escape inside a string
        assert_eq!(
            split_statements("SELECT 'it''s; fine'"),
            vec!["SELECT 'it''s; fine'"]
        );
        // E'' with a backslash-escaped quote
        assert_eq!(
            split_statements(r"SELECT E'a\';b'; SELECT 2"),
            vec![r"SELECT E'a\';b'", "SELECT 2"]
        );
        // dollar quoting, tagged and bare
        assert_eq!(
            split_statements("SELECT $$x;y$$; SELECT $fn$a;b$fn$"),
            vec!["SELECT $$x;y$$", "SELECT $fn$a;b$fn$"]
        );
    }

    #[test]
    fn comments_are_opaque_and_empty_statements_dropped() {
        assert_eq!(
            split_statements("SELECT 1 -- ; not a split\n; SELECT 2"),
            vec!["SELECT 1 -- ; not a split", "SELECT 2"]
        );
        assert_eq!(
            split_statements("/* a; /* nested; */ b; */ SELECT 1"),
            vec!["/* a; /* nested; */ b; */ SELECT 1"]
        );
        // comment-only / whitespace-only chunks yield no statement
        assert_eq!(split_statements("-- only a comment\n; ;"), Vec::<String>::new());
        assert_eq!(split_statements(";;  ;"), Vec::<String>::new());
    }

    #[test]
    fn dollar_parameter_is_not_a_tag() {
        assert_eq!(
            split_statements("SELECT $1; SELECT 2"),
            vec!["SELECT $1", "SELECT 2"]
        );
    }

    #[test]
    fn tracks_transaction_status() {
        use super::advance_tx;
        use super::TxStatus::{Active, Failed, Idle};
        // open, then close
        assert_eq!(advance_tx(Idle, "BEGIN", true), Active);
        assert_eq!(advance_tx(Active, "COMMIT", true), Idle);
        assert_eq!(advance_tx(Active, "ROLLBACK", true), Idle);
        // a whole cycle in one batch nets out to idle
        assert_eq!(advance_tx(Idle, "BEGIN; UPDATE t SET x=1; COMMIT", true), Idle);
        // BEGIN left open across runs stays active
        assert_eq!(advance_tx(Idle, "BEGIN; SELECT 1", true), Active);
        // an error inside an open tx aborts it; in autocommit it stays idle
        assert_eq!(advance_tx(Active, "SELECT bad", false), Failed);
        assert_eq!(advance_tx(Idle, "SELECT bad", false), Idle);
        // BEGIN then an error -> aborted, tx still open
        assert_eq!(advance_tx(Idle, "BEGIN; SELECT bad", false), Failed);
        // recovery: ROLLBACK clears the aborted state; other stmts keep failing
        assert_eq!(advance_tx(Failed, "ROLLBACK", true), Idle);
        assert_eq!(advance_tx(Failed, "SELECT 1", false), Failed);
        // ROLLBACK TO SAVEPOINT un-aborts but keeps the tx open
        assert_eq!(advance_tx(Failed, "ROLLBACK TO SAVEPOINT sp", true), Active);
        // synonyms and a leading comment
        assert_eq!(advance_tx(Active, "END", true), Idle);
        assert_eq!(advance_tx(Idle, "START TRANSACTION", true), Active);
        assert_eq!(advance_tx(Active, "ABORT", true), Idle);
        assert_eq!(advance_tx(Idle, "-- go\nBEGIN", true), Active);
        // a DO block's inner BEGIN/END is dollar-quoted, not a transaction verb
        assert_eq!(advance_tx(Idle, "DO $$ BEGIN PERFORM 1; END $$", true), Idle);
    }
}
