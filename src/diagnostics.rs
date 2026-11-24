use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tracing_subscriber::{Layer, filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone)]
struct LogWriter(Arc<Mutex<RotatingLog>>);
struct RotatingLog {
    file: File,
    path: PathBuf,
    bytes: u64,
}

impl RotatingLog {
    fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self { file, path, bytes })
    }

    fn append(&mut self, data: &[u8]) -> io::Result<()> {
        if self.bytes > 0 && self.bytes.saturating_add(data.len() as u64) > MAX_LOG_BYTES {
            // Keep one previous file. Rotation happens under the writer lock.
            let previous = self.path.with_extension("log.1");
            self.file.flush()?;
            fs::copy(&self.path, &previous)?;
            self.file.set_len(0)?;
            self.bytes = 0;
        }
        self.file.write_all(data)?;
        self.bytes += data.len() as u64;
        self.file.flush()
    }
}

impl Write for LogWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log writer unavailable"))?
            .append(data)?;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log writer unavailable"))?
            .file
            .flush()
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

pub fn init(default_directory: PathBuf) {
    let directory = std::env::var_os("SOFTPHONE_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_directory);
    let path = directory.join("softphone.log");
    // Only opt in to application-owned fields. Upstream wire logs can contain
    // credentials, so broad RUST_LOG settings are deliberately not applied.
    let file_layer = match RotatingLog::open(path.clone()) {
        Ok(log) => Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_writer(LogWriter(Arc::new(Mutex::new(log))))
                .with_filter(Targets::new().with_target("softphone", tracing::Level::DEBUG)),
        ),
        Err(error) => {
            eprintln!("Could not open diagnostic log {}: {error}", path.display());
            None
        }
    };
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_filter(
                    Targets::new()
                        .with_target("softphone", tracing::Level::INFO)
                        .with_target("rvoip_audio_device", tracing::Level::ERROR),
                ),
        )
        .with(file_layer)
        .try_init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), routing_revision = "ack-hostname-v3", log_path = %path.display(), "Softphone session started");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_appends_across_sessions_and_rotates() {
        let path = std::env::temp_dir().join(format!(
            "softphone-log-test-{}/softphone.log",
            std::process::id()
        ));
        let mut log = RotatingLog::open(path.clone()).unwrap();
        log.append(b"first session\n").unwrap();
        drop(log);
        let mut log = RotatingLog::open(path.clone()).unwrap();
        log.append(b"second session\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "first session\nsecond session\n"
        );
        log.file.set_len(MAX_LOG_BYTES).unwrap();
        log.bytes = MAX_LOG_BYTES;
        log.append(b"rotated\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"rotated\n");
        assert_eq!(
            fs::metadata(path.with_extension("log.1")).unwrap().len(),
            MAX_LOG_BYTES
        );
        drop(log);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
