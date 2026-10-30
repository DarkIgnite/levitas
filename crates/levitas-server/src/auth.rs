//! Online mode: proving players are who they say, as vanilla does.
//!
//! The server sends its RSA public key and a random token; the client answers with a
//! shared secret and the token, both encrypted with that key, and asks Mojang's session
//! server to vouch for it. From then on the connection is encrypted with AES-128 in
//! CFB8 mode, the shared secret serving as key and IV. The server then asks the session
//! server whether that player joined with that secret, and gets their real UUID and
//! skin.

use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use aes::Aes128;
use cfb8::cipher::generic_array::GenericArray;
use cfb8::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use rsa::pkcs8::EncodePublicKey;
use rsa::rand_core::{OsRng, RngCore};
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, ReadBuf};

use crate::connection::invalid;

const SESSION_SERVER: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";

pub type Encryptor = cfb8::Encryptor<Aes128>;
pub type Decryptor = cfb8::Decryptor<Aes128>;

/// One of a profile's properties: the skin (`textures`), signed by Mojang.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// The server's key pair, made once at start.
pub struct Keys {
    private: RsaPrivateKey,
    /// The public key in DER form, as clients receive it.
    pub public_der: Vec<u8>,
}

pub fn keys() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(|| {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).expect("an RSA key");
        let public_der = private
            .to_public_key()
            .to_public_key_der()
            .expect("a DER public key")
            .into_vec();
        Keys {
            private,
            public_der,
        }
    })
}

impl Keys {
    pub fn decrypt(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        self.private
            .decrypt(Pkcs1v15Encrypt, data)
            .map_err(|e| invalid(format!("could not decrypt: {e}")))
    }
}

/// A fresh ID for a player's session, sent with their login.
pub fn session_id() -> u128 {
    let mut bytes = [0; 16];
    OsRng.fill_bytes(&mut bytes);
    u128::from_be_bytes(bytes)
}

pub fn verify_token() -> [u8; 4] {
    let mut token = [0; 4];
    OsRng.fill_bytes(&mut token);
    token
}

/// Both directions' ciphers for a shared secret.
pub fn ciphers(secret: &[u8]) -> io::Result<(Encryptor, Decryptor)> {
    let bad = |_| invalid("the shared secret is not 16 bytes".to_owned());
    Ok((
        Encryptor::new_from_slices(secret, secret).map_err(bad)?,
        Decryptor::new_from_slices(secret, secret).map_err(bad)?,
    ))
}

pub fn encrypt(cipher: &mut Encryptor, data: &mut [u8]) {
    for byte in data.chunks_mut(1) {
        cipher.encrypt_block_mut(GenericArray::from_mut_slice(byte));
    }
}

pub fn decrypt(cipher: &mut Decryptor, data: &mut [u8]) {
    for byte in data.chunks_mut(1) {
        cipher.decrypt_block_mut(GenericArray::from_mut_slice(byte));
    }
}

/// The hash the session server knows the login by: SHA-1 of the server ID (empty),
/// the shared secret and the public key, written as a signed hexadecimal number, the
/// way Java's `BigInteger.toString(16)` writes it.
pub fn server_hash(secret: &[u8], public_der: &[u8]) -> String {
    let mut sha = Sha1::new();
    sha.update(secret);
    sha.update(public_der);
    let mut digest: [u8; 20] = sha.finalize().into();
    let negative = digest[0] & 0x80 != 0;
    if negative {
        // Two's complement: invert and add one.
        let mut carry = true;
        for b in digest.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (sum, overflow) = b.overflowing_add(1);
                *b = sum;
                carry = overflow;
            }
        }
    }
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let trimmed = hex.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    if negative {
        format!("-{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// Asks Mojang whether `name` joined with `hash`; their UUID, exact name and
/// properties if so.
pub async fn has_joined(
    name: &str,
    hash: &str,
) -> io::Result<Option<(u128, String, Vec<Property>)>> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::new()
    });
    // Player names and hashes need no escaping, but nothing else gets into the URL.
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Ok(None);
    }
    let response = client
        .get(format!("{SESSION_SERVER}?username={name}&serverId={hash}"))
        .send()
        .await
        .map_err(|e| io::Error::other(format!("session server unreachable: {e}")))?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    let text = response
        .text()
        .await
        .map_err(|e| io::Error::other(format!("session server reply: {e}")))?;
    profile_from_reply(&text, name)
}

/// The UUID, exact name and properties in a session server reply.
fn profile_from_reply(text: &str, name: &str) -> io::Result<Option<(u128, String, Vec<Property>)>> {
    let json: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| invalid(format!("session server reply is not JSON: {e}")))?;
    let Some(uuid) = json["id"]
        .as_str()
        .and_then(|id| u128::from_str_radix(id, 16).ok())
    else {
        return Ok(None);
    };
    let name = json["name"].as_str().unwrap_or(name).to_owned();
    let properties = json["properties"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(Property {
                name: p["name"].as_str()?.to_owned(),
                value: p["value"].as_str()?.to_owned(),
                signature: p["signature"].as_str().map(str::to_owned),
            })
        })
        .collect();
    Ok(Some((uuid, name, properties)))
}

/// A reader that decrypts what it reads once a cipher is set.
pub struct Decrypting<R> {
    inner: R,
    pub cipher: Option<Decryptor>,
}

impl<R> Decrypting<R> {
    pub fn new(inner: R) -> Self {
        Decrypting {
            inner,
            cipher: None,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Decrypting<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let this = &mut *self;
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let (Poll::Ready(Ok(())), Some(cipher)) = (&result, this.cipher.as_mut()) {
            decrypt(cipher, &mut buf.filled_mut()[before..]);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The examples from wiki.vg's protocol encryption page, made with SHA-1 of a name.
    fn hash_of_name(name: &str) -> String {
        server_hash(name.as_bytes(), &[])
    }

    #[test]
    fn server_hashes_match_javas_signed_hex() {
        assert_eq!(
            hash_of_name("Notch"),
            "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
        );
        assert_eq!(
            hash_of_name("jeb_"),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
        assert_eq!(
            hash_of_name("simon"),
            "88e16a1019277b15d58faf0541e11910eb756f6"
        );
    }

    /// A reply shaped like the session server's (values shortened).
    #[test]
    fn session_server_replies_give_the_profile() {
        let reply = r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch",
            "properties":[{"name":"textures","value":"ZXlK","signature":"c2ln"}],
            "profileActions":[]}"#;
        let (uuid, name, properties) = profile_from_reply(reply, "notch").unwrap().unwrap();
        assert_eq!(uuid, 0x069a79f444e94726a5befca90e38aaf5);
        assert_eq!(name, "Notch");
        assert_eq!(
            properties,
            vec![Property {
                name: "textures".to_owned(),
                value: "ZXlK".to_owned(),
                signature: Some("c2ln".to_owned()),
            }]
        );
        assert!(profile_from_reply("{}", "x").unwrap().is_none());
    }

    #[test]
    fn the_cipher_streams_round_trip() {
        let secret = [7u8; 16];
        let (mut enc, mut dec) = ciphers(&secret).unwrap();
        let mut data = b"hello, encrypted world".to_vec();
        let original = data.clone();
        encrypt(&mut enc, &mut data[..5]);
        encrypt(&mut enc, &mut data[5..]);
        assert_ne!(data, original);
        decrypt(&mut dec, &mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn keys_decrypt_what_their_public_half_encrypts() {
        use rsa::pkcs8::DecodePublicKey;
        let keys = keys();
        let public = rsa::RsaPublicKey::from_public_key_der(&keys.public_der).unwrap();
        let sealed = public
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"secret")
            .unwrap();
        assert_eq!(keys.decrypt(&sealed).unwrap(), b"secret");
    }
}
