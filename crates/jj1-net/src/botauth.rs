//! Signed requests ("Web Bot Auth").
//!
//! Websites block most automated clients because they cannot tell a
//! well-behaved agent from a scraper pretending to be one. Web Bot Auth (an
//! IETF draft built on HTTP Message Signatures, RFC 9421) lets a client sign
//! each request with a key the site can check against a published key
//! directory. A site can then allow "JJ1, run by this person" while still
//! blocking anonymous bots.
//!
//! JJ1 signs `@authority` (and `signature-agent` when a key directory is
//! configured) with Ed25519 on every request and every redirect hop:
//!
//! ```text
//! Signature-Agent: "https://you.example"
//! Signature-Input: sig1=("@authority" "signature-agent");created=...;expires=...;keyid="...";alg="ed25519";nonce="...";tag="web-bot-auth"
//! Signature: sig1=:<base64 signature>:
//! ```
//!
//! Signing never replaces honesty: JJ1 still sends its own User-Agent and
//! still respects robots.txt.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use url::Url;

/// How long a signature stays valid.
const VALID_SECS: u64 = 300;

#[derive(Clone)]
pub struct BotAuth {
    key: SigningKey,
    /// URL of the key directory's origin (sent as `Signature-Agent`).
    agent: Option<String>,
    keyid: String,
}

impl std::fmt::Debug for BotAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotAuth").field("keyid", &self.keyid).field("agent", &self.agent).finish_non_exhaustive()
    }
}

/// Load the 32-byte Ed25519 seed at `path`, creating a new random one the
/// first time.
pub fn load_or_create_seed(path: &std::path::Path) -> Result<[u8; 32], String> {
    if let Ok(bytes) = std::fs::read(path) {
        return bytes.try_into().map_err(|_| format!("{} is not a 32-byte key", path.display()));
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, seed).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(seed)
}

impl BotAuth {
    pub fn new(seed: [u8; 32], agent: Option<String>) -> BotAuth {
        let key = SigningKey::from_bytes(&seed);
        let keyid = thumbprint(&key.verifying_key());
        let agent = agent.map(|a| a.trim().trim_end_matches('/').to_string()).filter(|a| !a.is_empty());
        BotAuth { key, agent, keyid }
    }

    /// RFC 7638 JWK thumbprint of the public key; the `keyid` sites look up.
    pub fn keyid(&self) -> &str {
        &self.keyid
    }

    /// The public key as a JWK Set, to publish at
    /// `<agent>/.well-known/http-message-signatures-directory`.
    pub fn directory_json(&self) -> String {
        let x = URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes());
        format!(r#"{{"keys":[{{"kty":"OKP","crv":"Ed25519","x":"{x}","kid":"{}","use":"sig"}}]}}"#, self.keyid)
    }

    /// The headers to add to a request for `url`, signed now.
    pub fn headers(&self, url: &Url) -> Vec<(&'static str, String)> {
        let created = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut nonce = [0u8; 32];
        let _ = getrandom::getrandom(&mut nonce);
        self.headers_at(url, created, &STANDARD.encode(nonce))
    }

    fn headers_at(&self, url: &Url, created: u64, nonce: &str) -> Vec<(&'static str, String)> {
        let (base, input) = self.signature_base(url, created, nonce);
        let sig = STANDARD.encode(self.key.sign(base.as_bytes()).to_bytes());
        let mut h = Vec::new();
        if let Some(a) = &self.agent {
            h.push(("signature-agent", format!("\"{a}\"")));
        }
        h.push(("signature-input", format!("sig1={input}")));
        h.push(("signature", format!("sig1=:{sig}:")));
        h
    }

    /// The RFC 9421 signature base and the `Signature-Input` member value.
    fn signature_base(&self, url: &Url, created: u64, nonce: &str) -> (String, String) {
        let authority = match (url.host_str(), url.port()) {
            (Some(h), Some(p)) => format!("{}:{p}", h.to_ascii_lowercase()),
            (Some(h), None) => h.to_ascii_lowercase(),
            _ => String::new(),
        };
        let components = if self.agent.is_some() { r#"("@authority" "signature-agent")"# } else { r#"("@authority")"# };
        let params = format!(
            r#"{components};created={created};expires={};keyid="{}";alg="ed25519";nonce="{nonce}";tag="web-bot-auth""#,
            created + VALID_SECS,
            self.keyid
        );
        let mut base = format!("\"@authority\": {authority}\n");
        if let Some(a) = &self.agent {
            base.push_str(&format!("\"signature-agent\": \"{a}\"\n"));
        }
        base.push_str(&format!("\"@signature-params\": {params}"));
        (base, params)
    }
}

fn thumbprint(key: &VerifyingKey) -> String {
    let x = URL_SAFE_NO_PAD.encode(key.as_bytes());
    // Members in lexicographic order, no whitespace (RFC 7638).
    let jwk = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
    URL_SAFE_NO_PAD.encode(Sha256::digest(jwk.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    #[test]
    fn signature_base_matches_rfc9421_and_verifies() {
        let auth = BotAuth::new([7u8; 32], Some("https://agent.example/".into()));
        let url = Url::parse("https://Shop.Example:8443/cart?x=1").unwrap();
        let (base, input) = auth.signature_base(&url, 1_700_000_000, "abc");
        assert_eq!(
            base,
            format!(
                "\"@authority\": shop.example:8443\n\"signature-agent\": \"https://agent.example\"\n\"@signature-params\": (\"@authority\" \"signature-agent\");created=1700000000;expires=1700000300;keyid=\"{}\";alg=\"ed25519\";nonce=\"abc\";tag=\"web-bot-auth\"",
                auth.keyid()
            )
        );
        let h = auth.headers_at(&url, 1_700_000_000, "abc");
        assert_eq!(h[0], ("signature-agent", "\"https://agent.example\"".to_string()));
        assert_eq!(h[1].1, format!("sig1={input}"));
        let b64 = h[2].1.trim_start_matches("sig1=:").trim_end_matches(':');
        let sig = Signature::from_slice(&STANDARD.decode(b64).unwrap()).unwrap();
        auth.key.verifying_key().verify(base.as_bytes(), &sig).expect("signature verifies");
    }

    #[test]
    fn without_a_directory_only_the_authority_is_signed() {
        let auth = BotAuth::new([1u8; 32], None);
        let h = auth.headers(&Url::parse("https://a.test/").unwrap());
        assert_eq!(h.len(), 2);
        assert!(h[0].1.starts_with("sig1=(\"@authority\");created="));
        assert!(auth.directory_json().contains(auth.keyid()));
        assert_eq!(auth.keyid().len(), 43);
    }
}
