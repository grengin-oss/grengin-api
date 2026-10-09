// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

const READ_ONLY_START_KEYWORDS: &[&str] =
    &["SELECT", "SHOW", "DESC", "DESCRIBE", "EXPLAIN", "WITH"];

const WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "UPSERT", "REPLACE", "MERGE", "CREATE", "ALTER", "DROP",
    "TRUNCATE", "GRANT", "REVOKE", "COMMIT", "ROLLBACK", "BEGIN", "START", "VACUUM", "ANALYZE",
    "ATTACH", "DETACH", "PRAGMA", "EXEC", "EXECUTE", "CALL", "DO", "SET", "USE", "LOCK", "UNLOCK",
    "INTO",
];

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Semicolon(usize),
    Other,
}

struct Tokenizer<'a> {
    sql: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(sql: &'a str) -> Self {
        Self { sql, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.sql[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.bump() {
            if c == '\n' {
                break;
            }
        }
    }

    // PostgreSQL block comments nest, unlike most other dialects.
    fn skip_block_comment(&mut self) {
        self.pos += 2;
        let mut depth = 1usize;
        while depth > 0 && self.pos < self.sql.len() {
            if self.rest().starts_with("/*") {
                depth += 1;
                self.pos += 2;
            } else if self.rest().starts_with("*/") {
                depth -= 1;
                self.pos += 2;
            } else {
                self.bump();
            }
        }
    }

    // With standard_conforming_strings (the default since PostgreSQL 9.1) a backslash only
    // escapes inside E'' strings; treating it as an escape elsewhere lets "'\'" hide SQL.
    fn skip_quoted(&mut self, quote: char, backslash_escapes: bool) {
        self.bump();
        while let Some(c) = self.bump() {
            if backslash_escapes && c == '\\' {
                self.bump();
            } else if c == quote {
                if self.peek() == Some(quote) {
                    self.bump();
                } else {
                    return;
                }
            }
        }
    }

    fn dollar_quote_tag(&self) -> Option<&'a str> {
        let rest = self.rest();
        let after_dollar = &rest[1..];
        let tag_len = after_dollar
            .char_indices()
            .find(|&(i, c)| !(is_ident_char(c) && (i > 0 || !c.is_ascii_digit())))
            .map(|(i, _)| i)?;
        after_dollar[tag_len..]
            .starts_with('$')
            .then(|| &rest[..tag_len + 2])
    }

    fn skip_dollar_quoted(&mut self, tag: &str) {
        self.pos += tag.len();
        match self.rest().find(tag) {
            Some(end) => self.pos += end + tag.len(),
            None => self.pos = self.sql.len(),
        }
    }

    fn read_word(&mut self) -> String {
        let start = self.pos;
        while self.peek().is_some_and(|c| is_ident_char(c) || c == '$') {
            self.bump();
        }
        self.sql[start..self.pos].to_ascii_uppercase()
    }
}

impl Iterator for Tokenizer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        loop {
            let c = self.peek()?;
            let rest = self.rest();
            if c.is_whitespace() {
                self.bump();
            } else if rest.starts_with("--") {
                self.skip_line_comment();
            } else if rest.starts_with("/*") {
                self.skip_block_comment();
            } else if c == '\'' {
                self.skip_quoted('\'', false);
                return Some(Token::Other);
            } else if c == '"' {
                self.skip_quoted('"', false);
                return Some(Token::Other);
            } else if c == '$' {
                match self.dollar_quote_tag() {
                    Some(tag) => self.skip_dollar_quoted(tag),
                    None => {
                        self.bump();
                    }
                }
                return Some(Token::Other);
            } else if c == ';' {
                let offset = self.pos;
                self.bump();
                return Some(Token::Semicolon(offset));
            } else if c.is_ascii_digit() {
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == '_')
                {
                    self.bump();
                }
                return Some(Token::Other);
            } else if is_ident_char(c) {
                let word = self.read_word();
                if word == "E" && self.peek() == Some('\'') {
                    self.skip_quoted('\'', true);
                    return Some(Token::Other);
                }
                return Some(Token::Word(word));
            } else {
                self.bump();
                return Some(Token::Other);
            }
        }
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn is_read_only_sql(sql: &str) -> bool {
    let tokens: Vec<Token> = Tokenizer::new(sql).collect();
    let statement = match tokens
        .iter()
        .position(|token| matches!(token, Token::Semicolon(_)))
    {
        Some(pos) if pos + 1 < tokens.len() => return false,
        Some(pos) => &tokens[..pos],
        None => &tokens[..],
    };

    let mut words = statement.iter().filter_map(|token| match token {
        Token::Word(word) => Some(word.as_str()),
        _ => None,
    });
    let Some(first) = words.next() else {
        return false;
    };
    READ_ONLY_START_KEYWORDS.contains(&first) && !words.any(|word| WRITE_KEYWORDS.contains(&word))
}

pub fn statement_body(sql: &str) -> &str {
    Tokenizer::new(sql)
        .find_map(|token| match token {
            Token::Semicolon(offset) => Some(offset),
            _ => None,
        })
        .map_or(sql, |offset| &sql[..offset])
        .trim()
}

#[cfg(test)]
mod tests {
    use super::{is_read_only_sql, statement_body};

    #[test]
    fn accepts_select() {
        assert!(is_read_only_sql("SELECT * FROM users"));
    }

    #[test]
    fn accepts_select_with_comment_and_semicolon() {
        assert!(is_read_only_sql("SELECT * FROM users; -- trailing"));
    }

    #[test]
    fn rejects_multi_statement() {
        assert!(!is_read_only_sql(
            "SELECT * FROM users; SELECT * FROM orders"
        ));
    }

    #[test]
    fn rejects_write_statement() {
        assert!(!is_read_only_sql("UPDATE users SET name = 'x'"));
    }

    #[test]
    fn rejects_cte_with_write() {
        assert!(!is_read_only_sql(
            "WITH x AS (DELETE FROM users RETURNING *) SELECT * FROM x"
        ));
    }

    #[test]
    fn rejects_write_hidden_behind_a_backslash_in_a_standard_string() {
        assert!(!is_read_only_sql(
            r"WITH x AS (SELECT '\'), d AS (DELETE FROM users RETURNING 1) SELECT 1 --')"
        ));
    }

    #[test]
    fn rejects_statement_smuggled_after_a_backslash_in_a_standard_string() {
        assert!(!is_read_only_sql(r"SELECT 'a\'; DROP TABLE users; --'"));
    }

    #[test]
    fn accepts_escaped_quote_in_an_e_string() {
        assert!(is_read_only_sql(r"SELECT E'it\'s; DELETE' AS note"));
    }

    #[test]
    fn rejects_write_after_an_e_string_ending_in_an_escaped_backslash() {
        assert!(!is_read_only_sql(r"SELECT E'\\'; DELETE FROM users"));
    }

    #[test]
    fn keywords_inside_doubled_quote_strings_are_ignored() {
        assert!(is_read_only_sql(
            "SELECT 'it''s; DELETE FROM users' AS note"
        ));
    }

    #[test]
    fn keywords_inside_dollar_quoted_strings_are_ignored() {
        assert!(is_read_only_sql("SELECT $$ DELETE FROM users; $$ AS note"));
        assert!(is_read_only_sql("SELECT $tag$ DROP TABLE x $tag$"));
    }

    #[test]
    fn rejects_write_after_a_dollar_quoted_string() {
        assert!(!is_read_only_sql("SELECT $tag$ x $tag$; DROP TABLE users"));
    }

    #[test]
    fn positional_parameters_are_not_dollar_quotes() {
        assert!(is_read_only_sql(
            "SELECT * FROM users WHERE id = $1 AND org = $2"
        ));
        assert!(!is_read_only_sql(
            "SELECT $1; DELETE FROM users WHERE id = $1"
        ));
    }

    #[test]
    fn identifiers_containing_dollar_signs_do_not_open_a_dollar_quote() {
        assert!(!is_read_only_sql("SELECT a$b$ FROM t; DELETE FROM t"));
    }

    #[test]
    fn rejects_select_into_which_creates_a_table() {
        assert!(!is_read_only_sql("SELECT * INTO users_copy FROM users"));
    }

    #[test]
    fn nested_block_comments_follow_postgres_rules() {
        assert!(is_read_only_sql(
            "/* outer /* inner */ DELETE FROM users */ SELECT 1"
        ));
        assert!(!is_read_only_sql(
            "/* note */ DELETE FROM users */ SELECT 1"
        ));
    }

    #[test]
    fn quoted_identifiers_named_like_keywords_are_allowed() {
        assert!(is_read_only_sql(r#"SELECT "delete", "update" FROM audit"#));
    }

    #[test]
    fn rejects_empty_and_comment_only_input() {
        assert!(!is_read_only_sql(""));
        assert!(!is_read_only_sql("-- nothing\n/* here */"));
    }

    #[test]
    fn handles_multibyte_text_before_the_terminator() {
        assert!(is_read_only_sql("SELECT 'héllo wörld'; -- done"));
        assert!(!is_read_only_sql("SELECT 'é'; DELETE FROM users"));
    }

    #[test]
    fn statement_body_drops_the_terminator_and_trailing_comments() {
        assert_eq!(statement_body("SELECT 'é' ; -- note"), "SELECT 'é'");
        assert_eq!(statement_body("  SELECT 1  "), "SELECT 1");
        assert_eq!(statement_body("SELECT ';' AS semi;"), "SELECT ';' AS semi");
    }
}
