// SPDX-License-Identifier: GPL-2.0

//! Demo request handler shared by the server binary and referenced by the
//! client documentation.  The wire format is deliberately simple, ASCII, and
//! self-describing so the demo is easy to follow:
//!
//! * `ECHO <text>`      -> `<text>`
//! * `ADD <a> <b>`      -> decimal sum
//! * `MUL <a> <b>`      -> decimal product
//! * `REVERSE <text>`   -> `<text>` reversed
//! * `PING`             -> `PONG`
//!
//! Any malformed request yields an `ERR <reason>` reply.  A real service would
//! use a compact binary encoding; the point here is to exercise the transport.

pub fn handle(req: &[u8]) -> Vec<u8> {
    let text = match std::str::from_utf8(req) {
        Ok(t) => t.trim_end_matches(['\n', '\r']),
        Err(_) => return b"ERR non-utf8-request".to_vec(),
    };

    let mut parts = text.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");

    match cmd {
        "PING" => b"PONG".to_vec(),
        "ECHO" => rest.as_bytes().to_vec(),
        "REVERSE" => rest.chars().rev().collect::<String>().into_bytes(),
        "ADD" | "MUL" => match parse_two(rest) {
            Some((a, b)) => {
                let r = if cmd == "ADD" {
                    a.wrapping_add(b)
                } else {
                    a.wrapping_mul(b)
                };
                r.to_string().into_bytes()
            }
            None => b"ERR expected two integers".to_vec(),
        },
        "" => b"ERR empty-request".to_vec(),
        other => format!("ERR unknown-command {other}").into_bytes(),
    }
}

fn parse_two(rest: &str) -> Option<(i64, i64)> {
    let mut it = rest.split_whitespace();
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b))
}
