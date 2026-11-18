use std::{
    collections::HashSet,
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;

use crate::core::Account;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Config {
    pub group_codes: bool,
    pub show_notes: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            group_codes: true,
            show_notes: true,
        }
    }
}

pub struct Store {
    dir: PathBuf,
    // Keep this handle open until every in-memory edit has finished.
    _lock: File,
    pub accounts: Vec<Account>,
    pub config: Config,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("无法读取 {}：{err}", path.display())),
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|err| {
        format!(
            "无法解析 {}（第 {} 行，第 {} 列）",
            path.display(),
            err.line(),
            err.column()
        )
    })
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), String> {
    let write = || -> io::Result<()> {
        let mut temp = NamedTempFile::new_in(path.parent().expect("data file has a parent"))?;
        serde_json::to_writer_pretty(&mut temp, value)?;
        temp.write_all(b"\n")?;
        temp.as_file().sync_all()?;
        // On both Windows and Linux, the existing destination is replaced
        // without a period where the file is missing or partially written.
        temp.persist(path)?;
        Ok(())
    };
    write().map_err(|err| format!("无法保存 {}：{err}", path.display()))
}

fn validate_accounts(accounts: &[Account]) -> Result<Vec<Account>, String> {
    let mut ids = HashSet::with_capacity(accounts.len());
    let mut result = Vec::with_capacity(accounts.len());
    for (i, account) in accounts.iter().enumerate() {
        if !ids.insert(&account.id) {
            return Err(format!("第 {} 个条目的标识与其他条目重复", i + 1));
        }
        result.push(
            account
                .normalized()
                .map_err(|err| format!("第 {} 个条目：{err}", i + 1))?,
        );
    }
    Ok(result)
}

impl Store {
    pub fn open_portable() -> Result<Self, String> {
        let exe = env::current_exe().map_err(|err| format!("无法定位可执行文件：{err}"))?;
        let parent = exe.parent().ok_or("无法定位可执行文件所在目录")?;
        Self::open_dir(parent.join("data"))
    }

    pub(crate) fn open_dir(dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&dir).map_err(|err| format!("无法创建 {}：{err}", dir.display()))?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(".otpleaf.lock"))
            .map_err(|err| format!("无法打开 {} 中的锁文件：{err}", dir.display()))?;
        lock.try_lock().map_err(|err| match err {
            std::fs::TryLockError::WouldBlock => {
                format!("另一个 OTPLeaf 实例正在使用 {}", dir.display())
            }
            std::fs::TryLockError::Error(err) => format!("无法锁定 {}：{err}", dir.display()),
        })?;

        let accounts_path = dir.join("accounts.json");
        let config_path = dir.join("config.json");
        let existing_accounts: Option<Vec<Account>> = read_json(&accounts_path)?;
        let existing_config: Option<Config> = read_json(&config_path)?;
        let accounts = validate_accounts(existing_accounts.as_deref().unwrap_or_default())
            .map_err(|err| format!("{}：{err}", accounts_path.display()))?;
        if existing_accounts.is_none() {
            write_json(&accounts_path, &accounts)?;
        }
        let config = match existing_config {
            Some(config) => config,
            None => {
                let config = Config::default();
                write_json(&config_path, &config)?;
                config
            }
        };
        Ok(Self {
            dir,
            _lock: lock,
            accounts,
            config,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.dir
    }

    pub fn save_accounts(&mut self, accounts: Vec<Account>) -> Result<(), String> {
        let next = validate_accounts(&accounts)?;
        write_json(&self.dir.join("accounts.json"), &next)?;
        self.accounts = next;
        Ok(())
    }

    pub fn save_config(&mut self, config: Config) -> Result<(), String> {
        write_json(&self.dir.join("config.json"), &config)?;
        self.config = config;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn portable_round_trip_and_exclusive_session() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let mut store = Store::open_dir(path.clone()).unwrap();
        assert!(store.accounts.is_empty());
        assert!(store.config.group_codes && store.config.show_notes);
        assert!(Store::open_dir(path.clone()).is_err());
        let account = Account {
            secret: "m y".into(),
            name: " Test ".into(),
            ..Default::default()
        };
        let id = account.id.clone();
        store.save_accounts(vec![account]).unwrap();
        let config = Config {
            group_codes: false,
            show_notes: true,
        };
        store.save_config(config.clone()).unwrap();
        drop(store);
        let mut reopened = Store::open_dir(path.clone()).unwrap();
        assert_eq!(reopened.accounts[0].id, id);
        assert_eq!(reopened.accounts[0].name, "Test");
        assert_eq!(reopened.accounts[0].secret, "MY");
        assert_eq!(reopened.config, config);
        reopened.save_accounts(Vec::new()).unwrap();
        drop(reopened);
        assert!(Store::open_dir(path).unwrap().accounts.is_empty());
    }

    #[test]
    fn malformed_files_are_preserved() {
        for file in ["accounts.json", "config.json"] {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join(file), b"invalid").unwrap();
            assert!(Store::open_dir(dir.path().into()).is_err());
            assert_eq!(fs::read(dir.path().join(file)).unwrap(), b"invalid");
        }
    }

    #[test]
    fn rejected_updates_keep_previous_state() {
        let dir = tempdir().unwrap();
        let mut store = Store::open_dir(dir.path().to_path_buf()).unwrap();
        let account = Account {
            secret: "MY".into(),
            name: "A".into(),
            ..Default::default()
        };
        store.save_accounts(vec![account.clone()]).unwrap();
        let before = fs::read(dir.path().join("accounts.json")).unwrap();
        assert!(store.save_accounts(vec![account.clone(), account]).is_err());
        assert_eq!(fs::read(dir.path().join("accounts.json")).unwrap(), before);
        assert_eq!(store.accounts.len(), 1);

        // Simulate a destination the atomic replace cannot overwrite.
        let file = dir.path().join("config.json");
        fs::remove_file(&file).unwrap();
        fs::create_dir(file).unwrap();
        assert!(
            store
                .save_config(Config {
                    group_codes: false,
                    show_notes: false
                })
                .is_err()
        );
        assert_eq!(store.config, Config::default());
    }
}
