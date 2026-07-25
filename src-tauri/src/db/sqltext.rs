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

/// If `i` starts something a scanner must step over as one unit — a comment, a
/// string literal, a quoted identifier or a dollar-quoted body — returns the
/// offset just past it plus whether it counts as statement content (comments
/// don't). Single place, so every scanner here agrees where code ends and text
/// begins: `$1` and a bare `$` fall through as ordinary bytes.
fn skip_noise(b: &[u8], i: usize) -> Option<(usize, bool)> {
    match b[i] {
        b'-' if b.get(i + 1) == Some(&b'-') => {
            let mut i = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            Some((i, false))
        }
        b'/' if b.get(i + 1) == Some(&b'*') => {
            let mut depth = 1;
            let mut i = i + 2;
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
            Some((i.min(b.len()), false))
        }
        b'\'' => {
            // E'…' also understands backslash escapes
            let estring = i > 0
                && (b[i - 1] == b'e' || b[i - 1] == b'E')
                && (i < 2 || !(b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_'));
            let mut i = i + 1;
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
            Some((i.min(b.len()), true))
        }
        b'"' => {
            let mut i = i + 1;
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
            Some((i.min(b.len()), true))
        }
        b'$' => {
            // $tag$ … $tag$; $1 (a digit after $) is a parameter, not a tag
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if !(j < b.len() && b[j] == b'$' && (j == i + 1 || !b[i + 1].is_ascii_digit())) {
                return None;
            }
            let tag = &b[i..=j];
            let mut i = j + 1;
            while i + tag.len() <= b.len() && &b[i..i + tag.len()] != tag {
                i += 1;
            }
            Some(((i + tag.len()).min(b.len()), true))
        }
        _ => None,
    }
}

/// psql meta-commands (`\c`, `\i`, `\!`, …) found where the backslash is code
/// rather than text, returned without the backslash and in order. psql reads
/// them from the same stream as the SQL, so a batch piped to `psql -f -` can do
/// things no SQL parser sees: reconnect (losing the surrounding transaction),
/// include a file that never passed a gate, shell out. Skipping strings keeps
/// `WHERE x ~ '\d+'` from reading as a command.
pub fn psql_meta_commands(sql: &str) -> Vec<String> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some((next, _)) = skip_noise(b, i) {
            i = next;
            continue;
        }
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        // `\dt+`, `\?` — an alphanumeric run; `\!`, `\.` — one punctuation byte.
        let mut j = i + 1;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'+' || b[j] == b'?') {
            j += 1;
        }
        if j == i + 1 && b.get(j).is_some_and(u8::is_ascii) {
            j += 1;
        }
        out.push(sql[i + 1..j].to_string());
        i = j.max(i + 1);
    }
    out
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
        if let Some((next, content)) = skip_noise(b, i) {
            has_token |= content;
            i = next;
            continue;
        }
        match b[i] {
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
            // ROLLBACK TO [SAVEPOINT] keeps the tx open (and un-aborts it);
            // plain ROLLBACK ends it
            "ROLLBACK" if w1 == "TO" => {
                if s == TxStatus::Failed {
                    TxStatus::Active
                } else {
                    s
                }
            }
            // `… AND CHAIN` ends the tx but instantly opens a new one with the
            // same characteristics; outside a tx it's a warning-only no-op
            "COMMIT" | "END" | "ABORT" | "ROLLBACK" => {
                if s != TxStatus::Idle && chains_new_tx(&stmt) {
                    TxStatus::Active
                } else {
                    TxStatus::Idle
                }
            }
            _ => s,
        };
    }
    s
}

/// True when every statement in `sql` is pure transaction control
/// (`COMMIT`/`ROLLBACK`/`END`/`ABORT`) — the only thing a read-only caller is
/// allowed to run to *close* a transaction that a previous `--write` call left
/// open. Empty / comment-only input is false (nothing to run, nothing to
/// permit). `ROLLBACK TO SAVEPOINT` is intentionally excluded: it keeps the
/// (read-write) transaction open, so it must not slip past the read-only gate.
/// So is `… AND CHAIN` (see [`chains_new_tx`]): it instantly opens a new
/// transaction with the SAME (read-write) characteristics.
pub fn is_tx_control_only(sql: &str) -> bool {
    let stmts = split_statements(sql);
    !stmts.is_empty()
        && stmts.iter().all(|s| {
            let (w0, w1) = head_keywords(s);
            let closes = matches!(w0.as_str(), "COMMIT" | "END" | "ABORT")
                || (w0 == "ROLLBACK" && w1 != "TO");
            closes && !chains_new_tx(s)
        })
}

/// True when the batch can leave the transaction block it runs in, i.e. escape
/// an explicit `BEGIN READ ONLY` (see [`execute_read_only`](super::execute_read_only)).
///
/// This gate exists because `default_transaction_read_only` is a USERSET GUC:
/// the client switches it off at will, so it states an intent and never a
/// boundary. A read-only *transaction* is a real boundary — Postgres rejects
/// both writes and `SET TRANSACTION READ WRITE` (SQLSTATE 25001) inside one —
/// but only for as long as the batch stays inside it. `COMMIT; SET
/// default_transaction_read_only = off; INSERT …` would otherwise walk straight
/// out of the block and write.
///
/// Unlike "detect every write", this is a closed grammar: transaction control
/// is a finite, enumerable set of commands, so the gate can be exhaustive
/// rather than a best-effort blacklist. Statements that stay *inside* the block
/// and cannot lift its access mode are deliberately allowed — `SAVEPOINT`, a
/// nested `BEGIN` (a no-op warning), `SET search_path`, `SET statement_timeout`.
/// `… AND CHAIN` is allowed for the same reason it is refused by
/// [`is_tx_control_only`]: it opens a new transaction with the *same*
/// characteristics, which here means read-only.
pub fn escapes_read_only_tx(sql: &str) -> bool {
    split_statements(sql).iter().any(|stmt| {
        let (w0, w1) = head_keywords(stmt);
        match w0.as_str() {
            "COMMIT" | "END" | "ABORT" => !chains_new_tx(stmt),
            // ROLLBACK TO [SAVEPOINT] stays inside the block; plain ROLLBACK ends it
            "ROLLBACK" => w1 != "TO" && !chains_new_tx(stmt),
            // two-phase commit ends the transaction exactly as COMMIT does
            "PREPARE" => w1 == "TRANSACTION",
            // DISCARD ALL resets the session, read-only defaults included (it
            // errors inside a transaction block, but the exec path runs psql
            // statement by statement, where it would land)
            "DISCARD" => true,
            // `SET TRANSACTION READ WRITE` (and its GUC spelling `SET
            // transaction_read_only = off`) lifts the block's read-only mode
            // outright. Postgres only refuses it once the transaction has taken
            // a snapshot, so a batch opening with it would otherwise escalate
            // before the first real statement — see the `SELECT 1` in
            // [`execute_read_only`](super::execute_read_only).
            "SET" | "RESET" => {
                w1 == "TRANSACTION"
                    || w1 == "ALL"
                    || mentions_read_only_guc(stmt)
                    || (w1 == "SESSION" && words_of(stmt).iter().any(|w| w == "CHARACTERISTICS"))
            }
            _ => false,
        }
    })
}

/// Identifier-ish words of a statement, uppercased — keyword sniffing past the
/// first two words (`SET SESSION CHARACTERISTICS AS TRANSACTION …`).
fn words_of(stmt: &str) -> Vec<String> {
    strip_leading_noise(stmt)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_uppercase())
        .collect()
}

/// True when the statement names one of the read-only GUCs. Both spellings are
/// refused on the read-only path: `transaction_read_only` lifts the current
/// block, and `default_transaction_read_only` — harmless on its own, since each
/// call opens its own block — is refused too, so that "the read-only path never
/// touches the read-only knobs" stays a rule with no exceptions to reason about.
fn mentions_read_only_guc(stmt: &str) -> bool {
    words_of(stmt)
        .iter()
        .any(|w| w == "TRANSACTION_READ_ONLY" || w == "DEFAULT_TRANSACTION_READ_ONLY")
}

/// True for `COMMIT/ROLLBACK/END/ABORT … AND CHAIN` (PG 12+): the statement
/// ends the current transaction but immediately starts a new one with the same
/// characteristics — including read-write mode, which is why the read-only gate
/// must not treat it as a plain close. `AND NO CHAIN` is a plain close.
fn chains_new_tx(stmt: &str) -> bool {
    let words: Vec<String> = strip_leading_noise(stmt)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_uppercase())
        .collect();
    words.windows(2).any(|w| w[0] == "AND" && w[1] == "CHAIN")
}

#[cfg(test)]
mod tests {
    use super::{escapes_read_only_tx, is_tx_control_only, psql_meta_commands, split_statements};

    #[test]
    fn read_only_tx_escape_gate() {
        // the bypass this gate exists for: end the block, then write
        assert!(escapes_read_only_tx(
            "COMMIT; SET default_transaction_read_only = off; INSERT INTO t VALUES (1)"
        ));
        assert!(escapes_read_only_tx("SELECT 1; ROLLBACK"));
        assert!(escapes_read_only_tx("  end  "));
        assert!(escapes_read_only_tx("ABORT;"));
        assert!(escapes_read_only_tx("PREPARE TRANSACTION 'x'"));
        assert!(escapes_read_only_tx("DISCARD ALL"));
        assert!(escapes_read_only_tx("-- sneaky\nCOMMIT"));
        // lifting the block's access mode — every spelling
        assert!(escapes_read_only_tx("SET TRANSACTION READ WRITE; INSERT INTO t VALUES (1)"));
        assert!(escapes_read_only_tx("SET transaction_read_only = off"));
        assert!(escapes_read_only_tx("SET LOCAL transaction_read_only TO false"));
        assert!(escapes_read_only_tx("SET default_transaction_read_only = off"));
        assert!(escapes_read_only_tx(
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE"
        ));
        assert!(escapes_read_only_tx("RESET ALL"));
        assert!(escapes_read_only_tx("RESET transaction_read_only"));
        // plain reads, and statements that stay inside the block
        assert!(!escapes_read_only_tx("SELECT 1"));
        assert!(!escapes_read_only_tx("SET search_path = app; SELECT 1"));
        assert!(!escapes_read_only_tx("SET statement_timeout = '5s'"));
        assert!(!escapes_read_only_tx("RESET search_path"));
        assert!(!escapes_read_only_tx("SAVEPOINT sp; ROLLBACK TO SAVEPOINT sp"));
        // a nested BEGIN is a warning-only no-op, the block still holds
        assert!(!escapes_read_only_tx("BEGIN; SELECT 1"));
        // AND CHAIN reopens with the same (read-only) characteristics
        assert!(!escapes_read_only_tx("COMMIT AND CHAIN"));
        assert!(escapes_read_only_tx("COMMIT AND NO CHAIN"));
        // PREPARE without TRANSACTION is a prepared statement, not tx control
        assert!(!escapes_read_only_tx("PREPARE p AS SELECT 1"));
        // the verbs must not be matched inside strings or dollar-quoted bodies
        assert!(!escapes_read_only_tx("SELECT 'COMMIT'"));
        assert!(!escapes_read_only_tx("DO $$ BEGIN PERFORM 1; END $$"));
        assert!(!escapes_read_only_tx(""));
    }

    #[test]
    fn psql_meta_commands_seen_only_as_code() {
        // the escapes a SQL-only gate misses: reconnect, include, shell, \gexec
        assert_eq!(psql_meta_commands("SELECT 1; \\c postgres"), ["c"]);
        assert_eq!(psql_meta_commands("\\i /etc/passwd"), ["i"]);
        assert_eq!(psql_meta_commands("\\! id"), ["!"]);
        assert_eq!(psql_meta_commands("SELECT 'x' \\gexec"), ["gexec"]);
        assert_eq!(psql_meta_commands("\\dt+"), ["dt+"]);
        assert_eq!(psql_meta_commands("\\c a\n\\i b"), ["c", "i"]);
        // a backslash inside text is text: regex literals must keep working
        assert!(psql_meta_commands("SELECT * FROM t WHERE x ~ '\\d+'").is_empty());
        assert!(psql_meta_commands("SELECT E'\\\\c nope'").is_empty());
        assert!(psql_meta_commands("SELECT \"we\\ird\"").is_empty());
        assert!(psql_meta_commands("SELECT $$a\\c b$$").is_empty());
        assert!(psql_meta_commands("-- \\c nope\nSELECT 1").is_empty());
        assert!(psql_meta_commands("/* \\i x */ SELECT 1").is_empty());
        assert!(psql_meta_commands("SELECT 1").is_empty());
        // a trailing backslash must not slice past the end or spin
        assert_eq!(psql_meta_commands("SELECT 1 \\"), [""]);
    }

    #[test]
    fn tx_control_only_gate() {
        // pure transaction-closing statements — allowed for a read-only caller
        assert!(is_tx_control_only("COMMIT"));
        assert!(is_tx_control_only("ROLLBACK"));
        assert!(is_tx_control_only("  end  "));
        assert!(is_tx_control_only("ABORT;"));
        assert!(is_tx_control_only("-- done\nCOMMIT"));
        assert!(is_tx_control_only("ROLLBACK; COMMIT"));
        // anything that reads or writes — blocked
        assert!(!is_tx_control_only("SELECT 1"));
        assert!(!is_tx_control_only("DELETE FROM t"));
        assert!(!is_tx_control_only("COMMIT; SELECT 1"));
        // ROLLBACK TO keeps the (read-write) tx open — must not pass the gate
        assert!(!is_tx_control_only("ROLLBACK TO SAVEPOINT sp"));
        // AND CHAIN instantly opens a new tx with the same (read-write)
        // characteristics — not a plain close, must not pass either
        assert!(!is_tx_control_only("COMMIT AND CHAIN"));
        assert!(!is_tx_control_only("ROLLBACK AND CHAIN"));
        assert!(!is_tx_control_only("END WORK AND CHAIN"));
        assert!(is_tx_control_only("COMMIT AND NO CHAIN"));
        // nothing to run
        assert!(!is_tx_control_only(""));
        assert!(!is_tx_control_only("  -- just a comment"));
    }

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
        // AND CHAIN: the old tx ends, a new one with the same characteristics
        // opens immediately — the session never returns to idle
        assert_eq!(advance_tx(Active, "COMMIT AND CHAIN", true), Active);
        assert_eq!(advance_tx(Active, "ROLLBACK AND CHAIN", true), Active);
        assert_eq!(advance_tx(Failed, "COMMIT WORK AND CHAIN", true), Active);
        assert_eq!(advance_tx(Active, "COMMIT AND NO CHAIN", true), Idle);
        // outside a tx AND CHAIN is a warning-only no-op — nothing opens
        assert_eq!(advance_tx(Idle, "COMMIT AND CHAIN", true), Idle);
        // synonyms and a leading comment
        assert_eq!(advance_tx(Active, "END", true), Idle);
        assert_eq!(advance_tx(Idle, "START TRANSACTION", true), Active);
        assert_eq!(advance_tx(Active, "ABORT", true), Idle);
        assert_eq!(advance_tx(Idle, "-- go\nBEGIN", true), Active);
        // a DO block's inner BEGIN/END is dollar-quoted, not a transaction verb
        assert_eq!(advance_tx(Idle, "DO $$ BEGIN PERFORM 1; END $$", true), Idle);
    }
}
