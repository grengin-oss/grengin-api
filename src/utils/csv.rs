// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

/// RFC 4180 CSV reader with support for quoted fields and flexible row lengths.
pub struct CsvReader<'a> {
    data: &'a [u8],
    pos: usize,
    flexible: bool,
    header: Option<Vec<String>>,
}

impl<'a> CsvReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            flexible: false,
            header: None,
        }
    }

    pub fn flexible(mut self, v: bool) -> Self {
        self.flexible = v;
        self
    }

    /// Parse and cache the first row as column headers.
    /// Returns `None` when the input is empty.
    pub fn headers(&mut self) -> Option<Vec<String>> {
        if self.header.is_none() {
            self.header = self.parse_record();
        }
        self.header.clone()
    }

    /// Return all data rows (excluding the header row).
    /// Consumes the header first if `headers()` has not yet been called.
    pub fn records(&mut self) -> Vec<Vec<String>> {
        if self.header.is_none() {
            self.headers();
        }
        let mut out = Vec::new();
        while let Some(record) = self.parse_record() {
            if record.iter().any(|f| !f.trim().is_empty()) {
                out.push(record);
            }
        }
        out
    }

    fn parse_field(&mut self) -> Option<String> {
        if self.pos >= self.data.len() {
            return None;
        }
        if self.data[self.pos] == b'"' {
            self.pos += 1;
            let mut field = String::new();
            loop {
                if self.pos >= self.data.len() {
                    break;
                }
                if self.data[self.pos] == b'"' {
                    self.pos += 1;
                    if self.pos < self.data.len() && self.data[self.pos] == b'"' {
                        // Escaped double-quote inside a quoted field
                        field.push('"');
                        self.pos += 1;
                    } else {
                        break;
                    }
                } else {
                    field.push(self.data[self.pos] as char);
                    self.pos += 1;
                }
            }
            Some(field)
        } else {
            let start = self.pos;
            while self.pos < self.data.len()
                && self.data[self.pos] != b','
                && self.data[self.pos] != b'\r'
                && self.data[self.pos] != b'\n'
            {
                self.pos += 1;
            }
            Some(String::from_utf8_lossy(&self.data[start..self.pos]).into_owned())
        }
    }

    fn parse_record(&mut self) -> Option<Vec<String>> {
        if self.pos >= self.data.len() {
            return None;
        }
        let mut record = Vec::new();
        loop {
            let field = self.parse_field()?;
            record.push(field);
            if self.pos >= self.data.len() {
                break;
            }
            match self.data[self.pos] {
                b',' => {
                    self.pos += 1;
                }
                b'\r' => {
                    self.pos += 1;
                    if self.pos < self.data.len() && self.data[self.pos] == b'\n' {
                        self.pos += 1;
                    }
                    break;
                }
                b'\n' => {
                    self.pos += 1;
                    break;
                }
                _ => break,
            }
        }
        if record.is_empty() {
            None
        } else {
            Some(record)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_csv() {
        let data = b"a,b,c\n1,2,3\n4,5,6\n";
        let mut r = CsvReader::new(data).flexible(true);
        assert_eq!(r.headers(), Some(vec!["a".into(), "b".into(), "c".into()]));
        let rows = r.records();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1", "2", "3"]);
        assert_eq!(rows[1], vec!["4", "5", "6"]);
    }

    #[test]
    fn handles_quoted_fields() {
        let data = b"\"hello, world\",b\n\"foo\"\"\",bar\n";
        let mut r = CsvReader::new(data).flexible(true);
        let headers = r.headers().unwrap();
        assert_eq!(headers[0], "hello, world");
        let rows = r.records();
        assert_eq!(rows[0][0], "foo\"");
    }

    #[test]
    fn flexible_mode_allows_variable_rows() {
        let data = b"a,b,c\n1,2\n3,4,5,6\n";
        let mut r = CsvReader::new(data).flexible(true);
        r.headers();
        let rows = r.records();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[1].len(), 4);
    }

    #[test]
    fn handles_crlf_line_endings() {
        let data = b"a,b\r\n1,2\r\n";
        let mut r = CsvReader::new(data).flexible(true);
        assert_eq!(r.headers(), Some(vec!["a".into(), "b".into()]));
        assert_eq!(r.records().len(), 1);
    }

    #[test]
    fn empty_input() {
        let mut r = CsvReader::new(b"").flexible(true);
        assert!(r.headers().is_none());
        assert!(r.records().is_empty());
    }

    #[test]
    fn skips_empty_rows() {
        let data = b"a,b\n1,2\n\n3,4\n";
        let mut r = CsvReader::new(data).flexible(true);
        r.headers();
        assert_eq!(r.records().len(), 2);
    }
}
