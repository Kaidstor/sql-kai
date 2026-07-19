//! Small shared output-formatting helpers used by both the streaming export
//! (`db::export`) and the CLI's stdout renderer (`bin/sql-kai-cli/output`).

/// RFC 4180 CSV field: quote when the value contains a comma, quote, CR or LF,
/// doubling any embedded quote. Matches `lib/export.ts`; a NULL cell is passed
/// in already as an empty string.
pub fn csv_field(v: &str) -> String {
    if v.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field(""), "");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(csv_field("carriage\rreturn"), "\"carriage\rreturn\"");
        // embedded quotes are doubled and the field is wrapped
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
