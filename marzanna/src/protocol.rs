//! Marzanna tunneling protocol.
//!
//! Wire format is fairly simple:
//! ```
//! MTP FRAME:
//! ---------------------------------------------------------------------------------------
//! | preamble | salt | length ciphertext | length tag | payload ciphertext | payload tag |
//! ---------------------------------------------------------------------------------------
//! ```
//!
//! Preamble is 24 bits: 0000'0000'1111'1111'1110'1011
//!
//! Salt is used to derive the message encryption key:
//!
//!    subkey <- HKDF(<session key>, <salt>, <info>)
//!
//! and then ciphertexts are generated like so:
//!
//!    (length_ciphertext, length_tag) <- AE_encrypt(<subkey>, 0, length)
//!    (payload_ciphertext, payload_tag) <- AE_encrypt(<subkey>, 1, payload)
//!
//! We separate the length from the payload so that we can easily check each frame for validity
//! using only (salt, length_ct, length_tag).
