//! 接続先ごとのログ保持（リングバッファ）と、秘密情報のマスク処理。

use std::collections::{HashMap, VecDeque};

use crate::model::{LogLine, LogSource, LogStream};

/// 1接続先あたりの最大行数。超えたら古い行から捨てる
pub const MAX_LINES: usize = 5000;

/// マスク後の文字列
pub const MASK: &str = "********";

/// 接続先IDごとのログ。スレッド間の共有は呼び出し側（ConnectionManager）がロックで守る
#[derive(Debug, Default)]
pub struct LogBuffer {
    lines: HashMap<String, VecDeque<LogLine>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 1行追加する。MAX_LINES を超えたら古い行を捨てる
    pub fn push(&mut self, line: LogLine) {
        let q = self.lines.entry(line.target_id.clone()).or_default();
        while q.len() >= MAX_LINES {
            q.pop_front();
        }
        q.push_back(line);
    }

    pub fn get(&self, id: &str) -> Vec<LogLine> {
        self.lines
            .get(id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn len(&self, id: &str) -> usize {
        self.lines.get(id).map_or(0, |q| q.len())
    }

    pub fn is_empty(&self, id: &str) -> bool {
        self.len(id) == 0
    }

    pub fn clear(&mut self, id: &str) {
        if let Some(q) = self.lines.get_mut(id) {
            q.clear();
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.lines.remove(id);
    }
}

/// 現在のローカル時刻（HH:MM:SS）
pub fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// ログ1行を作る（時刻は現在時刻）。text はマスク済みであること
pub fn make_line(target_id: &str, source: LogSource, stream: LogStream, text: &str) -> LogLine {
    LogLine {
        target_id: target_id.to_string(),
        time: now_hms(),
        source,
        stream,
        text: text.to_string(),
    }
}

/// JSONで値をマスクするキー（大文字小文字は区別しない）
const SECRET_KEYS: &[&str] = &["password", "secretstring"];

/// 秘密情報をマスクする。
/// - `known` に含まれる値（空文字は除く）をすべて `********` に置き換える
/// - JSON風の `"password": "..."` と `"SecretString": "..."` の値を置き換える
///   （エスケープされたJSON `\"password\":\"...\"` も対象）
pub fn mask_secrets(text: &str, known: &[&str]) -> String {
    let mut out = text.to_string();
    for k in known {
        if !k.is_empty() && out.contains(k) {
            out = out.replace(k, MASK);
        }
    }
    mask_json_values(&out)
}

/// `"key"` の直後の文字列値をマスクする（正規表現を使わない手書きの走査）
fn mask_json_values(text: &str) -> String {
    let bytes = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let lower = lower.as_bytes();
    let mut out = String::with_capacity(text.len());
    // out へ書き出し済みの位置
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched = None;
        if bytes[i] == b'"' {
            for key in SECRET_KEYS {
                if let Some((start, end)) = match_secret_value(bytes, lower, i, key.as_bytes()) {
                    matched = Some((start, end));
                    break;
                }
            }
        }
        match matched {
            Some((start, end)) => {
                // start..end が値の中身（引用符を除く）。キー位置などはASCIIなので文字境界
                out.push_str(&text[copied..start]);
                out.push_str(MASK);
                copied = end;
                i = end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// bytes[q] が `"` のとき、`"key"`（またはエスケープ形 `\"key\"`）の後に続く文字列値の範囲を返す
fn match_secret_value(bytes: &[u8], lower: &[u8], q: usize, key: &[u8]) -> Option<(usize, usize)> {
    // 開き引用符の直前がバックスラッシュならエスケープされたJSON
    let escaped = q > 0 && bytes[q - 1] == b'\\';
    let key_start = q + 1;
    let key_end = key_start + key.len();
    if key_end > bytes.len() || &lower[key_start..key_end] != key {
        return None;
    }
    let mut p = key_end;
    // 閉じ引用符
    if escaped {
        if bytes.get(p) != Some(&b'\\') {
            return None;
        }
        p += 1;
    }
    if bytes.get(p) != Some(&b'"') {
        return None;
    }
    p += 1;
    p = skip_ws(bytes, p);
    if bytes.get(p) != Some(&b':') {
        return None;
    }
    p = skip_ws(bytes, p + 1);
    // 値の開き引用符
    if escaped {
        if bytes.get(p) != Some(&b'\\') {
            return None;
        }
        p += 1;
    }
    if bytes.get(p) != Some(&b'"') {
        return None;
    }
    let start = p + 1;
    let mut j = start;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if escaped => {
                // エスケープ形では `\"` が終端。`\\` は1文字分として飛ばす
                match bytes.get(j + 1) {
                    Some(b'"') => return Some((start, j)),
                    Some(b'\\') => j += 2,
                    _ => j += 1,
                }
            }
            b'\\' => j += 2,
            b'"' if !escaped => return Some((start, j)),
            _ => j += 1,
        }
    }
    // 終端がない（行の途中で切れている）場合は行末までマスク
    Some((start, bytes.len()))
}

fn skip_ws(bytes: &[u8], mut p: usize) -> usize {
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, text: &str) -> LogLine {
        make_line(id, LogSource::App, LogStream::Stdout, text)
    }

    #[test]
    fn buffer_caps_and_drops_oldest() {
        let mut b = LogBuffer::new();
        for i in 0..(MAX_LINES + 10) {
            b.push(line("a", &format!("line {i}")));
        }
        b.push(line("b", "other"));
        let a = b.get("a");
        assert_eq!(a.len(), MAX_LINES);
        assert_eq!(a[0].text, "line 10");
        assert_eq!(a[MAX_LINES - 1].text, format!("line {}", MAX_LINES + 9));
        assert_eq!(b.len("b"), 1);
        b.clear("a");
        assert!(b.is_empty("a"));
        assert_eq!(b.len("b"), 1);
        b.remove("b");
        assert!(b.get("b").is_empty());
        assert!(b.get("unknown").is_empty());
    }

    #[test]
    fn time_format() {
        let t = now_hms();
        assert_eq!(t.len(), 8);
        assert_eq!(&t[2..3], ":");
    }

    #[test]
    fn masks_known_values() {
        assert_eq!(
            mask_secrets("pw is s3cret! ok s3cret!", &["s3cret!", ""]),
            "pw is ******** ok ********"
        );
        assert_eq!(mask_secrets("nothing", &[]), "nothing");
    }

    #[test]
    fn masks_json_password() {
        assert_eq!(
            mask_secrets(r#"{"username":"admin","password":"p@ss\"w"}"#, &[]),
            r#"{"username":"admin","password":"********"}"#
        );
        assert_eq!(
            mask_secrets(r#"{ "Password" : "abc", "x": 1 }"#, &[]),
            r#"{ "Password" : "********", "x": 1 }"#
        );
    }

    #[test]
    fn masks_secret_string() {
        let s = r#"    "SecretString": "{\"username\":\"admin\",\"password\":\"x\"}","#;
        assert_eq!(mask_secrets(s, &[]), r#"    "SecretString": "********","#);
    }

    #[test]
    fn masks_escaped_password() {
        let s = r#"error: {\"username\":\"admin\",\"password\":\"p\\w\"} end"#;
        assert_eq!(
            mask_secrets(s, &[]),
            r#"error: {\"username\":\"admin\",\"password\":\"********\"} end"#
        );
    }

    #[test]
    fn masks_unterminated_value() {
        assert_eq!(
            mask_secrets(r#""password": "abc"#, &[]),
            r#""password": "********"#
        );
    }

    #[test]
    fn leaves_other_text() {
        let s = r#"password is required; "passwordPolicy": "x"; "password": 123; 日本語"#;
        assert_eq!(mask_secrets(s, &[]), s);
    }

    #[test]
    fn non_ascii_safe() {
        assert_eq!(
            mask_secrets(r#"日本語 "password":"秘密" あと"#, &[]),
            r#"日本語 "password":"********" あと"#
        );
    }
}
