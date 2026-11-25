//! AWS Signature Version 4 request signing, done on the host so no AWS key ever enters a guest.
//!
//! The guest's AWS SDK signs with placeholder credentials. For a destination the host holds a
//! SigV4 credential for, the egress gate removes the guest's signature headers and signs the
//! request again here, over the exact bytes that are sent.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &self.session_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Headers a guest may have set that belong to the signature. They are removed before the
/// host signs, so a guest cannot smuggle its own signature or token through.
pub const GUEST_SIGNATURE_HEADERS: &[&str] = &[
    "authorization",
    "x-amz-date",
    "x-amz-security-token",
    "x-amz-content-sha256",
];

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding as SigV4 requires: unreserved characters stay, everything else is %XX.
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || (b == b'/' && !encode_slash);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Canonical URI for every service except S3: each path segment of the (already encoded)
/// request path is encoded once more.
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    path.split('/').map(|seg| uri_encode(seg, true)).collect::<Vec<_>>().join("/")
}

fn decode_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn canonical_query(query: Option<&str>) -> String {
    let Some(q) = query.filter(|q| !q.is_empty()) else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (uri_encode(&decode_component(k), true), uri_encode(&decode_component(v), true))
        })
        .collect();
    pairs.sort();
    pairs.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// `YYYYMMDD'T'HHMMSS'Z'` for a Unix timestamp, without a date-time dependency.
pub fn amz_date(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        y,
        m,
        d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

pub fn now_amz_date() -> String {
    amz_date(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

pub struct SigningInput<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
    /// Headers to sign, as (lowercase name, value). Must include `host` and `x-amz-date`.
    pub headers: &'a [(String, String)],
    pub payload_sha256: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub amz_date: &'a str,
}

pub struct Signature {
    pub canonical_request: String,
    pub string_to_sign: String,
    pub authorization: String,
}

pub fn sign(input: &SigningInput<'_>, creds: &AwsCredentials) -> Signature {
    let mut headers: Vec<(String, String)> = input
        .headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method.to_ascii_uppercase(),
        canonical_uri(input.path),
        canonical_query(input.query),
        canonical_headers,
        signed_headers,
        input.payload_sha256
    );

    let date = &input.amz_date[..8];
    let scope = format!("{date}/{}/{}/aws4_request", input.region, input.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        input.amz_date,
        scope,
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac(format!("AWS4{}", creds.secret_access_key).as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, input.region.as_bytes());
    let k_service = hmac(&k_region, input.service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key_id
    );
    Signature { canonical_request, string_to_sign, authorization }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_creds() -> AwsCredentials {
        AwsCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        }
    }

    /// `get-vanilla` from the AWS SigV4 test suite.
    #[test]
    fn aws_suite_get_vanilla() {
        let headers = vec![
            ("Host".to_string(), "example.amazonaws.com".to_string()),
            ("X-Amz-Date".to_string(), "20150830T123600Z".to_string()),
        ];
        let sig = sign(
            &SigningInput {
                method: "GET",
                path: "/",
                query: None,
                headers: &headers,
                payload_sha256: &sha256_hex(b""),
                region: "us-east-1",
                service: "service",
                amz_date: "20150830T123600Z",
            },
            &example_creds(),
        );
        assert_eq!(
            sha256_hex(sig.canonical_request.as_bytes()),
            "bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
        );
        assert_eq!(
            sig.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn amz_date_formats_known_instants() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
        assert_eq!(amz_date(951_782_400), "20000229T000000Z"); // leap day
    }

    #[test]
    fn canonical_uri_double_encodes_segments() {
        // Bedrock model ids with ':' arrive encoded once by the SDK.
        assert_eq!(
            canonical_uri("/model/anthropic.claude-3-sonnet-20240229-v1%3A0/converse"),
            "/model/anthropic.claude-3-sonnet-20240229-v1%253A0/converse"
        );
        assert_eq!(canonical_uri(""), "/");
    }

    #[test]
    fn canonical_query_sorts_and_encodes() {
        assert_eq!(canonical_query(Some("b=2&a=1&c=x%20y")), "a=1&b=2&c=x%20y");
        assert_eq!(canonical_query(None), "");
    }

    #[test]
    fn debug_redacts_secrets() {
        let mut c = example_creds();
        c.session_token = Some("TOKEN".into());
        let s = format!("{c:?}");
        assert!(!s.contains("wJalr") && !s.contains("TOKEN"));
    }
}
