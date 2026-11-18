use std::{
    collections::HashSet,
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;

use crate::core::Account;

pub struct Store {
    dir: PathBuf,
    // Keep this handle open until every in-memory edit has finished.
    _lock: File,
    pub accounts: Vec<Account>,
}

fn read_accounts(path: &Path) -> Result<Option<Vec<Account>>, String> {
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

fn write_accounts(path: &Path, accounts: &[Account]) -> Result<(), String> {
    let write = || -> io::Result<()> {
        let mut temp = NamedTempFile::new_in(path.parent().expect("data file has a parent"))?;
        serde_json::to_writer_pretty(&mut temp, accounts)?;
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
        let accounts = match read_accounts(&accounts_path)? {
            Some(accounts) => validate_accounts(&accounts)
                .map_err(|err| format!("{}：{err}", accounts_path.display()))?,
            None => {
                write_accounts(&accounts_path, &[])?;
                Vec::new()
            }
        };
        Ok(Self {
            dir,
            _lock: lock,
            accounts,
        })
    }

    pub fn save_accounts(&mut self, accounts: Vec<Account>) -> Result<(), String> {
        let next = validate_accounts(&accounts)?;
        write_accounts(&self.dir.join("accounts.json"), &next)?;
        self.accounts = next;
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
        assert!(Store::open_dir(path.clone()).is_err());
        let account = Account {
            secret: "   jbsw y3dp   ehpk 3pxp  ".into(),
            name: " Test ".into(),
            ..Default::default()
        };
        let id = account.id.clone();
        let code = account.code_at(59).unwrap();
        store.save_accounts(vec![account]).unwrap();
        assert_eq!(store.accounts[0].secret, "JBSWY3DPEHPK3PXP");
        let bytes = fs::read(path.join("accounts.json")).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!([{
                "id": id, "name": "Test", "secret": "JBSWY3DPEHPK3PXP",
                "digits": 6, "period": 30, "algorithm": "SHA1"
            }])
        );
        drop(store);
        let mut reopened = Store::open_dir(path.clone()).unwrap();
        assert_eq!(reopened.accounts[0].id, id);
        assert_eq!(reopened.accounts[0].name, "Test");
        assert_eq!(reopened.accounts[0].secret, "JBSWY3DPEHPK3PXP");
        assert_eq!(reopened.accounts[0].code_at(59).unwrap(), code);
        reopened.save_accounts(Vec::new()).unwrap();
        drop(reopened);
        assert!(Store::open_dir(path).unwrap().accounts.is_empty());
    }

    #[test]
    fn malformed_accounts_are_preserved() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("accounts.json");
        fs::write(&file, b"invalid").unwrap();
        assert!(Store::open_dir(dir.path().into()).is_err());
        assert_eq!(fs::read(file).unwrap(), b"invalid");
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
        let file = dir.path().join("accounts.json");
        fs::remove_file(&file).unwrap();
        fs::create_dir(file).unwrap();
        assert!(store.save_accounts(Vec::new()).is_err());
        assert_eq!(store.accounts.len(), 1);
    }
}
