//! Encodings and addresses (modules `base64`, `hash` and `url`, C105), without a
//! dependency: Base64 (RFC 4648), SHA-256 (FIPS 180-4), MD5 (RFC 1321), HMAC (RFC 2104),
//! and web addresses cut and joined as RFC 3986 says.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The bytes in Base64, with `=` at the end; with `url`, `-` and `_` replace `+` and `/`,
/// without `=`.
pub fn base64_encode(bytes: &[u8], url: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                let c = ALPHABET[(n >> (18 - 6 * i) & 63) as usize];
                out.push(match (url, c) {
                    (true, b'+') => '-',
                    (true, b'/') => '_',
                    _ => c as char,
                });
            } else if !url {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes of a Base64 text, in either alphabet, with or without `=`; spaces and line
/// ends are skipped.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    let mut ended = false;
    for (i, c) in text.chars().enumerate() {
        let value = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => {
                ended = true;
                continue;
            }
            c if c.is_whitespace() => continue,
            c => return Err(format!("`{c}` at position {} is not Base64", i + 1)),
        };
        if ended {
            return Err("Base64 goes on after its `=`".to_string());
        }
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    if bits >= 6 {
        return Err("the Base64 text is cut short".to_string());
    }
    Ok(out)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of the bytes.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    for block in padded(bytes, true).chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// MD5 of the bytes: for the sums that sites still publish, never for security.
pub fn md5(bytes: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15,
        21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for block in padded(bytes, false).chunks(64) {
        let m: Vec<u32> = (0..16)
            .map(|i| u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().expect("4 bytes")))
            .collect();
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 16];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// The message with its `1` bit, zeros, and its length in bits on 64 bits.
fn padded(bytes: &[u8], big_endian: bool) -> Vec<u8> {
    let mut data = bytes.to_vec();
    let bits = (bytes.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend(if big_endian { bits.to_be_bytes() } else { bits.to_le_bytes() });
    data
}

/// HMAC-SHA-256 of the message with the key, as APIs sign their requests.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).chain(message.iter().copied()).collect();
    let outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).chain(sha256(&inner)).collect();
    sha256(&outer)
}

/// The parts of an absolute address: scheme, user, host, port (empty when not written),
/// path, query and fragment.
#[derive(Debug, PartialEq)]
pub struct Url {
    pub scheme: String,
    pub user: String,
    pub host: String,
    pub port: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// The parts of an absolute address such as `https://user@site.fr:8080/a/b?x=1#top`.
pub fn parse_url(text: &str) -> Result<Url, String> {
    let text = text.trim();
    let Some((scheme, rest)) = text.split_once("://") else {
        return Err(format!("`{text}` is not an absolute address, such as https://site.fr/page"));
    };
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
        return Err(format!("`{scheme}` is not a scheme, such as https"));
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let (user, host_port) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (host, port) = if host_port.starts_with('[') {
        // An IPv6 address: `[::1]:8080`.
        match host_port.find(']') {
            Some(end) => (&host_port[..=end], host_port[end + 1..].strip_prefix(':').unwrap_or("")),
            None => return Err(format!("`{host_port}` has no `]`")),
        }
    } else {
        host_port.rsplit_once(':').unwrap_or((host_port, ""))
    };
    if !port.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("`{port}` is not a port"));
    }
    Ok(Url {
        scheme: scheme.to_ascii_lowercase(),
        user: user.to_string(),
        host: host.to_ascii_lowercase(),
        port: port.to_string(),
        path: if path.is_empty() { "/".to_string() } else { path.to_string() },
        query: query.to_string(),
        fragment: fragment.to_string(),
    })
}

/// The address that `link` designates on the page at `base`, as a browser follows a link
/// (RFC 3986, §5.2).
pub fn join_url(base: &str, link: &str) -> Result<String, String> {
    let link = link.trim();
    if let Some((scheme, _)) = link.split_once(':')
        && !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        // Already absolute: `https://…`, `mailto:…`.
        return Ok(link.to_string());
    }
    let base = parse_url(base)?;
    let origin = |url: &Url| {
        let user = if url.user.is_empty() { String::new() } else { format!("{}@", url.user) };
        let port = if url.port.is_empty() { String::new() } else { format!(":{}", url.port) };
        format!("{}://{user}{}{port}", url.scheme, url.host)
    };
    if let Some(rest) = link.strip_prefix("//") {
        return Ok(format!("{}://{rest}", base.scheme));
    }
    let (link_rest, fragment) = link.split_once('#').map_or((link, None), |(a, b)| (a, Some(b)));
    let (link_path, query) = link_rest.split_once('?').map_or((link_rest, None), |(a, b)| (a, Some(b)));
    let path = if link_path.is_empty() {
        base.path.clone()
    } else if link_path.starts_with('/') {
        remove_dots(link_path)
    } else {
        let folder = &base.path[..base.path.rfind('/').map_or(0, |at| at + 1)];
        remove_dots(&format!("{folder}{link_path}"))
    };
    let query = match query {
        Some(query) => format!("?{query}"),
        None if link_path.is_empty() && !base.query.is_empty() => format!("?{}", base.query),
        None => String::new(),
    };
    let fragment = fragment.map_or(String::new(), |f| format!("#{f}"));
    Ok(format!("{}{path}{query}{fragment}", origin(&base)))
}

/// The path without its `.` and `..` segments.
fn remove_dots(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            segment => out.push(segment),
        }
    }
    let joined = out.join("/");
    if joined.starts_with('/') { joined } else { format!("/{joined}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_goes_both_ways() {
        assert_eq!(base64_encode(b"Lion!", false), "TGlvbiE=");
        assert_eq!(base64_encode(b"\xfb\xff", true), "-_8");
        assert_eq!(base64_decode("TGlvbiE=").unwrap(), b"Lion!");
        assert_eq!(base64_decode("TGlv\nbiE").unwrap(), b"Lion!");
        assert!(base64_decode("TG*v").is_err());
        assert!(base64_decode("T").is_err());
    }

    #[test]
    fn hashes_match_the_standards() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            hex(&md5(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        assert_eq!(
            hex(&hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog")),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        let long = "a".repeat(1000);
        assert_eq!(
            hex(&sha256(long.as_bytes())),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn cuts_and_joins_addresses() {
        let url = parse_url("HTTPS://moi@Site.fr:8080/a/b?x=1#top").unwrap();
        assert_eq!(
            (url.scheme.as_str(), url.user.as_str(), url.host.as_str(), url.port.as_str()),
            ("https", "moi", "site.fr", "8080")
        );
        assert_eq!((url.path.as_str(), url.query.as_str(), url.fragment.as_str()), ("/a/b", "x=1", "top"));
        assert_eq!(parse_url("http://[::1]:80").unwrap().host, "[::1]");
        assert!(parse_url("/relative").is_err());
        let base = "https://site.fr/a/b/c?q=1";
        for (link, expected) in [
            ("d", "https://site.fr/a/b/d"),
            ("../d", "https://site.fr/a/d"),
            ("/d?x", "https://site.fr/d?x"),
            ("//cdn.fr/x", "https://cdn.fr/x"),
            ("?y=2", "https://site.fr/a/b/c?y=2"),
            ("#top", "https://site.fr/a/b/c?q=1#top"),
            ("../../../d", "https://site.fr/d"),
            ("./", "https://site.fr/a/b/"),
            ("mailto:a@b.fr", "mailto:a@b.fr"),
        ] {
            assert_eq!(join_url(base, link).unwrap(), expected, "{link}");
        }
    }
}
