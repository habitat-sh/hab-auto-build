use std::{fmt::Display, fs::File, hash::Hash, io::Read, path::Path};

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(from = "String", into = "String")]
pub struct ShaSum(String);

impl ShaSum {
    pub fn from_path(path: impl AsRef<Path>) -> Result<ShaSum> {
        let mut hasher = Sha256::new();
        let mut file = File::open(path)?;
        let mut buffer = [0u8; 1024];
        while let Ok(n) = file.read(&mut buffer) {
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        let result = hasher.finalize();
        let shasum: String = result
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<String>>()
            .join("");
        Ok(ShaSum(shasum))
    }
}

impl AsRef<str> for ShaSum {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl From<String> for ShaSum {
    fn from(value: String) -> Self {
        ShaSum(value)
    }
}

impl From<ShaSum> for String {
    fn from(value: ShaSum) -> Self {
        value.0
    }
}

impl Display for ShaSum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(from = "String", into = "String")]
pub struct Blake3(String);

impl Blake3 {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Blake3> {
        let mut hasher = blake3::Hasher::new();
        let mut file = File::open(path)?;
        let mut buffer = [0u8; 4096];
        while let Ok(n) = file.read(&mut buffer) {
            if n == 0 {
                break;
            }
            hasher.update_rayon(&buffer[..n]);
        }
        let result = hasher.finalize();
        Ok(Blake3(result.to_string()))
    }
    pub fn hash_value(value: impl Serialize) -> Result<Blake3> {
        let mut hasher = blake3::Hasher::new();
        let value = serde_json::to_string(&value)?;
        hasher.update_rayon(value.as_bytes());
        let result = hasher.finalize();
        Ok(Blake3(result.to_string()))
    }
}

impl AsRef<str> for Blake3 {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl From<String> for Blake3 {
    fn from(value: String) -> Self {
        Blake3(value)
    }
}

impl From<Blake3> for String {
    fn from(value: Blake3) -> Self {
        value.0
    }
}

impl Display for Blake3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
