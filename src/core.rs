use std::{fmt, str::FromStr};

use data_encoding::{BASE32, BASE32_NOPAD};
use hmac::{Hmac, Mac, digest::KeyInit};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Algorithm {
    #[default]
    Sha1,
    Sha256,
    Sha512,
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sha1 => "SHA1",
            Self::Sha256 => "SHA256",
            Self::Sha512 => "SHA512",
        })
    }
}

impl FromStr for Algorithm {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_uppercase().as_str() {
            "SHA1" | "SHA-1" => Ok(Self::Sha1),
            "SHA256" | "SHA-256" => Ok(Self::Sha256),
            "SHA512" | "SHA-512" => Ok(Self::Sha512),
            _ => Err("算法须为 SHA1、SHA256 或 SHA512".into()),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub digits: u8,
    pub period: u32,
    pub algorithm: Algorithm,
    #[serde(default)]
    pub note: String,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            secret: String::new(),
            digits: 6,
            period: 30,
            algorithm: Algorithm::Sha1,
            note: String::new(),
        }
    }
}

fn decode_secret(secret: &str) -> Result<Vec<u8>, String> {
    let compact: String = secret.chars().filter(|c| !c.is_whitespace()).collect();
    let encoding = if compact.ends_with('=') {
        BASE32
    } else {
        BASE32_NOPAD
    };
    let bytes = encoding
        .decode(compact.to_ascii_uppercase().as_bytes())
        .map_err(|_| "密钥不是有效的 Base32，请检查 A–Z、2–7 和末尾填充".to_string())?;
    if bytes.is_empty() {
        return Err("请输入 Base32 密钥".into());
    }
    Ok(bytes)
}

fn truncate_hmac<M: Mac + KeyInit>(secret: &[u8], counter: u64) -> Result<u32, String> {
    let mut mac =
        <M as KeyInit>::new_from_slice(secret).map_err(|_| "无法读取 HMAC 密钥".to_string())?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let word = [
        digest[offset],
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ];
    Ok(u32::from_be_bytes(word) & 0x7fff_ffff)
}

impl Account {
    fn validate_parameters(&self) -> Result<(), String> {
        if !(6..=8).contains(&self.digits) {
            return Err("验证码位数须为 6、7 或 8".into());
        }
        if !(1..=i32::MAX as u32).contains(&self.period) {
            return Err("周期须为 1 至 2147483647 之间的整数秒".into());
        }
        Ok(())
    }

    pub fn normalized(&self) -> Result<Self, String> {
        if self.id.trim().is_empty() {
            return Err("条目标识不能为空".into());
        }
        if self.name.trim().is_empty() {
            return Err("请输入条目名称".into());
        }
        self.validate_parameters()?;
        Ok(Self {
            name: self.name.trim().to_owned(),
            secret: BASE32_NOPAD.encode(&decode_secret(&self.secret)?),
            ..self.clone()
        })
    }

    pub fn code_at(&self, unix_seconds: u64) -> Result<String, String> {
        self.validate_parameters()?;
        let counter = unix_seconds / u64::from(self.period);
        let secret = decode_secret(&self.secret)?;
        let value = match self.algorithm {
            Algorithm::Sha1 => truncate_hmac::<Hmac<Sha1>>(&secret, counter)?,
            Algorithm::Sha256 => truncate_hmac::<Hmac<Sha256>>(&secret, counter)?,
            Algorithm::Sha512 => truncate_hmac::<Hmac<Sha512>>(&secret, counter)?,
        };
        let code = value % 10_u32.pow(u32::from(self.digits));
        Ok(format!("{code:0width$}", width = usize::from(self.digits)))
    }

    pub fn remaining_at(&self, unix_seconds: u64) -> u32 {
        if self.period == 0 {
            return 0;
        }
        self.period - (unix_seconds % u64::from(self.period)) as u32
    }
}

pub fn parse_otpauth(input: &str) -> Result<Account, String> {
    let uri =
        Url::parse(input.trim()).map_err(|_| "请输入完整的 otpauth://totp/ 链接".to_string())?;
    if uri.scheme() != "otpauth"
        || !uri
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("totp"))
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some()
        || uri.fragment().is_some()
    {
        return Err("仅支持 otpauth://totp/ 类型的链接".into());
    }
    let label = percent_decode_str(uri.path().strip_prefix('/').unwrap_or_default())
        .decode_utf8()
        .map_err(|_| "链接中的条目名称编码无效".to_string())?;
    let label = label.trim();
    if label.is_empty() {
        return Err("链接中缺少条目名称".into());
    }
    let mut account = Account::default();
    let mut issuer = String::new();
    let mut seen = std::collections::HashSet::new();
    for (key, value) in uri.query_pairs() {
        let key = key.to_ascii_lowercase();
        if !matches!(
            key.as_str(),
            "secret" | "digits" | "period" | "algorithm" | "issuer"
        ) {
            continue;
        }
        if !seen.insert(key.clone()) {
            return Err(format!("链接中包含重复参数 {key}"));
        }
        match key.as_str() {
            "secret" => account.secret = value.into_owned(),
            "digits" => account.digits = value.parse().map_err(|_| "验证码位数须为 6、7 或 8")?,
            "period" => account.period = value.parse().map_err(|_| "周期须为正整数秒")?,
            "algorithm" => account.algorithm = value.parse()?,
            "issuer" => issuer = value.trim().to_owned(),
            _ => unreachable!(),
        }
    }
    account.name = if issuer.is_empty() || label.contains(':') {
        label.to_owned()
    } else {
        format!("{issuer}: {label}")
    };
    account.normalized()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_appendix_b() {
        let algorithms = [Algorithm::Sha1, Algorithm::Sha256, Algorithm::Sha512];
        let keys = [
            "12345678901234567890",
            "12345678901234567890123456789012",
            "1234567890123456789012345678901234567890123456789012345678901234",
        ];
        let vectors = [
            (59, ["94287082", "46119246", "90693936"]),
            (1_111_111_109, ["07081804", "68084774", "25091201"]),
            (1_111_111_111, ["14050471", "67062674", "99943326"]),
            (1_234_567_890, ["89005924", "91819424", "93441116"]),
            (2_000_000_000, ["69279037", "90698825", "38618901"]),
            (20_000_000_000, ["65353130", "77737706", "47863826"]),
        ];
        for (i, algorithm) in algorithms.into_iter().enumerate() {
            let account = Account {
                algorithm,
                secret: BASE32_NOPAD.encode(keys[i].as_bytes()),
                digits: 8,
                ..Default::default()
            };
            for (time, expected) in vectors {
                assert_eq!(account.code_at(time).unwrap(), expected[i]);
            }
        }
    }

    #[test]
    fn countdown_and_code_use_same_period() {
        let account = Account {
            secret: "MY======".into(),
            period: 45,
            ..Default::default()
        };
        assert_eq!(account.remaining_at(44), 1);
        assert_eq!(account.remaining_at(45), 45);
        assert_eq!(account.code_at(45).unwrap(), account.code_at(89).unwrap());
        assert_ne!(account.code_at(44).unwrap(), account.code_at(45).unwrap());
    }

    #[test]
    fn import_decodes_unicode_and_normalizes_fields() {
        let account = parse_otpauth("otpauth://totp/%E5%BC%A0%20San?secret=m%20y%3D%3D%3D%3D%3D%3D&issuer=Example&algorithm=sha-256&digits=8&period=45").unwrap();
        assert_eq!(account.name, "Example: 张 San");
        assert_eq!(account.secret, "MY");
        assert_eq!(account.algorithm, Algorithm::Sha256);
        assert_eq!((account.digits, account.period), (8, 45));
        let standard =
            parse_otpauth("otpauth://totp/Example%3Aalice?secret=MY&issuer=Example").unwrap();
        assert_eq!(standard.name, "Example:alice");
        assert_eq!(standard.digits, 6);
        assert_eq!(standard.period, 30);
        assert_eq!(standard.algorithm, Algorithm::Sha1);
    }

    #[test]
    fn invalid_links_and_parameters_are_rejected() {
        for invalid in [
            "https://example.com/",
            "otpauth://hotp/alice?secret=MY",
            "otpauth://totp/alice?secret=HELLO!",
            "otpauth://totp/?secret=MY",
            "otpauth://totp/alice",
            "otpauth://totp/alice?secret=MY&secret=ME",
            "otpauth://totp/alice?secret=MY&digits=20",
            "otpauth://totp/alice?secret=MY&period=0",
            "otpauth://totp/alice?secret=MY&period=2147483648",
            "otpauth://totp/alice?secret=MY&algorithm=MD5",
        ] {
            assert!(parse_otpauth(invalid).is_err());
        }
    }
}
