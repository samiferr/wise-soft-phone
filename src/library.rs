use crate::account::Account;
use chrono::Local;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Instant,
};

pub fn line_directory(account_path: &Path, account: &Account) -> PathBuf {
    account_path
        .with_file_name("lines")
        .join(format!("server-{}", account.registrar.replace(':', "_")))
        .join(format!("user-{}", account.username))
}

pub fn load<T: Default + DeserializeOwned>(path: &Path) -> Result<T, String> {
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| "Could not read saved phone data.".into())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(_) => Err("Could not open saved phone data.".into()),
    }
}

pub fn save(path: &Path, data: &impl Serialize) -> Result<(), String> {
    let write = || -> io::Result<()> {
        let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut tmp, data)?;
        tmp.persist(path)?;
        Ok(())
    };
    write().map_err(|_| "Could not save phone data on this device.".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallType {
    Incoming,
    Missed,
    Outgoing,
}

impl CallType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Incoming => "Incoming",
            Self::Missed => "Missed",
            Self::Outgoing => "Outgoing",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub peer: String,
    pub target: Option<String>,
    pub kind: CallType,
    pub timestamp: i64,
    pub seconds: Option<u64>,
}

pub struct CallLog {
    record: Record,
    connected: Option<Instant>,
}

impl CallLog {
    pub fn new(peer: String, target: Option<String>, incoming: bool) -> Self {
        Self {
            record: Record {
                peer,
                target,
                kind: if incoming {
                    CallType::Missed
                } else {
                    CallType::Outgoing
                },
                timestamp: Local::now().timestamp(),
                seconds: None,
            },
            connected: None,
        }
    }

    pub fn answer(&mut self) {
        if self.record.kind == CallType::Missed {
            self.record.kind = CallType::Incoming;
        }
        self.connected.get_or_insert_with(Instant::now);
    }

    pub fn stop_timer(&mut self) {
        if let Some(start) = self.connected.take() {
            self.record.seconds = Some(start.elapsed().as_secs());
        }
    }

    pub fn finish(mut self) -> Record {
        self.stop_timer();
        self.record
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AddressBook {
    pub name: String,
    pub description: String,
    pub contacts: Vec<Contact>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub target: String,
}
