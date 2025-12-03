//! Serialization for [`ChaCha20Rng`], for use with `#[serde(with="serde_chacha")]`.

use base64::{Engine as _, prelude::BASE64_STANDARD};
use rand::SeedableRng as _;
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

#[derive(Debug, Serialize, Deserialize)]
struct Vehicle {
    seed: String,
    stream: u64,
    word_pos: String,
}

pub fn serialize<S>(chacha: &ChaCha20Rng, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let (seed, stream, word_pos) = (
        chacha.get_seed(),
        chacha.get_stream(),
        chacha.get_word_pos(),
    );
    let v = Vehicle {
        seed: BASE64_STANDARD.encode(seed),
        stream: stream,
        word_pos: BASE64_STANDARD.encode(word_pos.to_le_bytes()),
    };
    v.serialize(serializer)
}
pub fn deserialize<'de, D>(deserializer: D) -> Result<ChaCha20Rng, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Vehicle::deserialize(deserializer)?;
    let (seed, stream, word_pos) = (
        *BASE64_STANDARD
            .decode(v.seed)
            .map_err(|e| D::Error::custom(format!("chacha8 seed must be base64-encoded: {e:?}")))?
            .as_array::<32>()
            .ok_or(D::Error::custom("chacha8 seed must be 32 bytes"))?,
        v.stream,
        *BASE64_STANDARD
            .decode(v.word_pos)
            .map_err(|e| {
                D::Error::custom(format!(
                    "chacha8 word position must be base64-encoded: {e:?}"
                ))
            })?
            .as_array::<16>()
            .ok_or(D::Error::custom("chacha8 word position must be 16 bytes"))?,
    );
    let mut chacha = ChaCha20Rng::from_seed(seed);
    chacha.set_stream(stream);
    chacha.set_word_pos(u128::from_le_bytes(word_pos));
    Ok(chacha)
}
