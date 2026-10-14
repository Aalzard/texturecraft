//! Tokens and bounded framing for the loopback JSON-lines transports (the app's control channel
//! and `texturecraft-cli serve` share the token conventions).

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{AutomationError, bad};

/// Environment variable holding the control token (both sides).
pub const TOKEN_ENV: &str = "TEXTURECRAFT_CONTROL_TOKEN";
/// Largest JSON request line, newline included (the app's limit too).
pub const MAX_REQUEST_BYTES: usize = 8 << 20;
/// Simultaneous TCP connections served by `serve --port`.
pub const MAX_CONNECTIONS: usize = 16;
/// Steps in one `command_batch`.
pub const MAX_BATCH_STEPS: usize = 256;

/// The token file the app (and `serve --port`) writes when the environment gives no token.
pub fn token_file(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!("texturecraft-control-{port}.token"))
}

/// The app accepts any environment token of at least 16 characters; generated ones are 64 hex.
fn validate_token(t: &str) -> Result<(), AutomationError> {
    if (16..=1024).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_graphic()) {
        Ok(())
    } else {
        Err(bad("control token must be 16 to 1024 printable ASCII characters"))
    }
}

/// A fresh 256-bit token as 64 hex digits, from the OS random generator.
pub fn generate_token() -> Result<String, AutomationError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| AutomationError::Other(format!("cannot generate a control token: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Token for a client of `port`: explicit, else `TEXTURECRAFT_CONTROL_TOKEN`, else the token file.
pub fn client_token(port: u16, supplied: Option<String>) -> Result<String, AutomationError> {
    let token = match supplied.or_else(|| std::env::var(TOKEN_ENV).ok()) {
        Some(t) => t,
        None => {
            let path = token_file(port);
            std::fs::read_to_string(&path)
                .map_err(|e| bad(format!("no control token: set {TOKEN_ENV} or start the app with `--control {port}` so it writes {} ({e})", path.display())))?
        }
    };
    let token = token.trim().to_string();
    validate_token(&token)?;
    Ok(token)
}

/// Token for a server on `port`: `TEXTURECRAFT_CONTROL_TOKEN`, else a fresh one written to
/// [`token_file`] (owner-only on Unix). Returns the token and where it was written.
pub fn server_token(port: u16) -> Result<(String, Option<PathBuf>), AutomationError> {
    if let Ok(t) = std::env::var(TOKEN_ENV) {
        validate_token(t.trim())?;
        return Ok((t.trim().to_string(), None));
    }
    let token = generate_token()?;
    let path = token_file(port);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let res = o.open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, token.as_bytes()));
    res.map_err(|e| AutomationError::Io(format!("cannot write the control token to {}: {e}", path.display())))?;
    Ok((token, Some(path)))
}

/// Compare equal-length tokens without an early exit.
pub fn token_matches(expected: &str, supplied: &str) -> bool {
    expected.len() == supplied.len() && expected.bytes().zip(supplied.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

pub enum LineRead {
    Eof,
    Line,
    /// Over [`MAX_REQUEST_BYTES`]; the rest of the line is still unread.
    TooLong,
}

/// Read one line, never buffering more than [`MAX_REQUEST_BYTES`] + 1 bytes.
pub fn read_bounded_line(r: &mut impl BufRead, line: &mut String) -> std::io::Result<LineRead> {
    line.clear();
    let mut bytes = Vec::new();
    let n = std::io::Read::take(&mut *r, (MAX_REQUEST_BYTES + 1) as u64).read_until(b'\n', &mut bytes)?;
    if n == 0 {
        return Ok(LineRead::Eof);
    }
    if n > MAX_REQUEST_BYTES {
        if bytes.last() != Some(&b'\n') {
            discard_rest_of_line(r)?;
        }
        return Ok(LineRead::TooLong);
    }
    *line = String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(LineRead::Line)
}

fn discard_rest_of_line(r: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(());
        }
        if let Some(end) = buf.iter().position(|&b| b == b'\n') {
            r.consume(end + 1);
            return Ok(());
        }
        let n = buf.len();
        r.consume(n);
    }
}

/// Counts live connections; a permit is released when dropped.
#[derive(Clone, Default)]
pub struct ConnectionLimiter(Arc<AtomicUsize>);

pub struct Permit(Arc<AtomicUsize>);

impl ConnectionLimiter {
    pub fn try_acquire(&self, max: usize) -> Option<Permit> {
        self.0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < max).then_some(n + 1)).ok().map(|_| Permit(self.0.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let a = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, generate_token().unwrap());
        assert!(token_matches(&a, &a.clone()));
        assert!(!token_matches(&a, "short"));
        assert!(client_token(1, Some("too short".into())).is_err());
        assert!(client_token(1, Some(format!(" {a}\n"))).is_ok());
    }

    #[test]
    fn oversized_lines_are_skipped() {
        let input = format!("{}\n{{\"ok\":1}}\n", "x".repeat(MAX_REQUEST_BYTES + 10));
        let mut r = std::io::Cursor::new(input);
        let mut line = String::new();
        assert!(matches!(read_bounded_line(&mut r, &mut line).unwrap(), LineRead::TooLong));
        assert!(matches!(read_bounded_line(&mut r, &mut line).unwrap(), LineRead::Line));
        assert_eq!(line, "{\"ok\":1}\n");
        assert!(matches!(read_bounded_line(&mut r, &mut line).unwrap(), LineRead::Eof));
    }

    #[test]
    fn limiter_releases() {
        let l = ConnectionLimiter::default();
        let p = l.try_acquire(1).unwrap();
        assert!(l.try_acquire(1).is_none());
        drop(p);
        assert!(l.try_acquire(1).is_some());
    }
}
